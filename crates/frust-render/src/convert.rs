//! Translation of the renderer-agnostic [`frust_scene::Scene`] display list
//! into `vello` draw calls.
//!
//! The mapping logic lives behind the [`SceneSink`] trait so it can be unit
//! tested without a GPU (see the tests below, which record calls into a plain
//! `Vec`). The only production implementor is `vello::Scene`; the public
//! [`encode_scene`] entry point is the one place a `vello` type appears in this
//! crate's API — deliberately, so a shell that owns its own `vello::Renderer`
//! can reuse Frust's scene encoding (mirrors how `frust-scene` allows
//! `peniko` types, spec §7).

use frust_scene::{Command, GlyphRun, PathStyle, Scene};
use kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Stroke};
use peniko::{Brush, Color, Fill, ImageData};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Sink for the individual draw operations a [`Scene`] decomposes into.
///
/// Implemented for `vello::Scene` (real rendering) and for a recording sink in
/// tests (GPU-free verification of the command mapping).
pub(crate) trait SceneSink {
    /// Fill an axis-aligned rectangle with `brush` under `transform`.
    fn fill_rect(&mut self, style: Fill, transform: Affine, brush: &Brush, rect: &Rect);
    /// Fill an axis-aligned rounded rectangle (uniform corner `radius`).
    fn fill_rounded_rect(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: &Brush,
        rect: &Rect,
        radius: f64,
    );
    /// Stroke a straight line segment from `p0` to `p1` with the given `width`.
    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64);
    /// Draw a positioned run of glyphs.
    fn draw_glyph_run(&mut self, run: &GlyphRun);
    /// Push a rectangular clip onto the backend's clip stack, under `transform`.
    fn push_clip(&mut self, transform: Affine, rect: &Rect);
    /// Pop the most recently pushed clip.
    fn pop_clip(&mut self);
    /// Draw a decoded image (natural pixel size `data.width`x`data.height`),
    /// scaled to fill `dest`, under `transform`.
    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect);
    /// Draw a gaussian-blurred rounded-rectangle elevation shadow.
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
/// path does exactly that (spec §7: rebuild the scene per frame, never
/// accumulate).
///
/// This is the no-shader-map convenience entry the public seam
/// (`docs/ARCHITECTURE.md`'s scene-layer purity rule) exposes for shells that
/// drive their own `vello::Renderer`: with no per-frame shader-override map,
/// every `Command::ShaderQuad` lowers to its miss placeholder (a CPU-tier /
/// no-prepass caller has no compiled shader targets anyway). The in-crate
/// render path calls [`encode_scene_with_shaders`] instead.
pub fn encode_scene(scene: &Scene, target: &mut vello::Scene) {
    encode_into(scene, target);
}

/// Encodes `scene` into `target`, resolving each [`Command::ShaderQuad`] against
/// `shader_images` (`(program id, clamped physical size)` → the shader pre-pass's
/// registered override [`ImageData`]). A hit lowers to a `draw_image`; a miss
/// keeps the placeholder fill. The in-crate render path
/// ([`crate::renderer::SurfaceRenderer::encode`]) builds the map in its shader
/// pre-pass and calls this, passing the same `adapter_max` the pre-pass used so
/// the per-quad key recomputed here matches the one the entry was stored under.
pub(crate) fn encode_scene_with_shaders(
    scene: &Scene,
    target: &mut vello::Scene,
    shader_images: &HashMap<(u64, u32, u32), ImageData>,
    adapter_max: u32,
) {
    encode_into_with_shaders(scene, target, shader_images, adapter_max);
}

/// Generic worker behind [`encode_scene`]; kept separate so tests (and the
/// `cpu-tier` sink) can drive it with a non-`vello` sink and no shader map. The
/// empty map means every [`Command::ShaderQuad`] misses to its placeholder, so
/// the `adapter_max` passed here is immaterial — `u32::MAX` (no extra clamp).
pub(crate) fn encode_into(scene: &Scene, sink: &mut impl SceneSink) {
    encode_into_with_shaders(scene, sink, &HashMap::new(), u32::MAX);
}

/// The command walk, parameterized by the per-frame shader-override map (empty
/// for the no-shader callers above) and the `adapter_max` used to recompute each
/// [`Command::ShaderQuad`]'s `(id, w, h)` map key (identical to the pre-pass's
/// keying). Kept generic over [`SceneSink`] so it is GPU-free unit-testable.
pub(crate) fn encode_into_with_shaders(
    scene: &Scene,
    sink: &mut impl SceneSink,
    shader_images: &HashMap<(u64, u32, u32), ImageData>,
    adapter_max: u32,
) {
    // Active clip/opacity group stack, tracked so a `ClearRect` can be HOISTED
    // to the root: a `Compose::Clear` inside a vello layer group only clears
    // that group's own accumulated content — anything painted OUTSIDE the
    // group (an app-root backdrop below a scroll_view's clip) survives the
    // group composite, defeating the Mode B hole punch (t11-redo defect D4,
    // pixel-proven in `tests/gpu_smoke.rs`). On `ClearRect` the walk pops
    // every open group, emits the clear at root — bounded by the intersection
    // of the popped groups' clip bounds so a partially-scrolled slot still
    // clips to its viewport — then re-pushes the same groups and continues.
    // Content painted before the slot (any nesting) is cleared; content
    // painted after (overlapping chrome) composites over the hole as before.
    // Caveat: an opacity group split this way composites its two halves
    // independently (a transient, transition-only artifact where translucent
    // group content overlaps a slot mid-animation — documented tradeoff).
    enum Group {
        Clip,
        Layer(f32),
    }
    let mut groups: Vec<(Group, Affine, Rect)> = Vec::new();

    for command in scene.commands() {
        match command {
            Command::FillRect {
                rect,
                brush,
                transform,
            } => sink.fill_rect(Fill::NonZero, *transform, brush, rect),
            Command::RoundedRect {
                rect,
                radius,
                brush,
                transform,
            } => sink.fill_rounded_rect(Fill::NonZero, *transform, brush, rect, *radius),
            Command::Line {
                p0,
                p1,
                width,
                brush,
                transform,
            } => sink.stroke_line(*transform, brush, *p0, *p1, *width),
            Command::GlyphRun(run) => sink.draw_glyph_run(run),
            Command::PushClip { rect, transform } => {
                groups.push((Group::Clip, *transform, *rect));
                sink.push_clip(*transform, rect);
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
                sink.draw_image(*transform, data, dest);
            }
            Command::BlurredRoundedRect {
                rect,
                radius,
                std_dev,
                color,
                transform,
            } => sink.draw_blurred_rounded_rect(*transform, rect, *color, *radius, *std_dev),
            Command::PushLayer {
                rect,
                alpha,
                transform,
            } => {
                groups.push((Group::Layer(*alpha), *transform, *rect));
                sink.push_layer(*transform, rect, *alpha);
            }
            Command::PopLayer => {
                groups.pop();
                sink.pop_layer();
            }
            Command::ClearRect { rect, transform } if groups.is_empty() => {
                sink.clear_rect(*transform, rect);
            }
            Command::ClearRect { rect, transform } => {
                // Hoist to root (see the `groups` doc above): bound the punch
                // by every open group's clip bbox, pop them all, clear, then
                // re-push. Bboxes are exact for the axis-aligned transforms
                // frust emits (translate/scale); a rotated clip would bound
                // conservatively.
                let mut punch = transform.transform_rect_bbox(*rect);
                for (_, t, r) in &groups {
                    punch = punch.intersect(t.transform_rect_bbox(*r));
                }
                if punch.width() > 0.0 && punch.height() > 0.0 {
                    for (kind, ..) in groups.iter().rev() {
                        match kind {
                            Group::Clip => sink.pop_clip(),
                            Group::Layer(_) => sink.pop_layer(),
                        }
                    }
                    sink.clear_rect(Affine::IDENTITY, &punch);
                    for (kind, t, r) in &groups {
                        match kind {
                            Group::Clip => sink.push_clip(*t, r),
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
            } => match style {
                PathStyle::Fill => sink.fill_path(*transform, brush, path),
                PathStyle::Stroke { width } => sink.stroke_path(*transform, brush, path, *width),
            },
            Command::ShaderQuad {
                program,
                dest,
                transform,
                time: _,
            } => {
                // Resolve the program's shader pre-pass output (an offscreen
                // texture registered with vello as an image override — see
                // `crate::renderer`'s pre-pass and RESEARCH.md §Q1). A hit
                // lowers to the same `draw_image` path a `Command::Image` uses,
                // reusing `natural_to_dest_transform`'s natural→dest scaling so
                // the physical-pixel target lands pixel-for-pixel in `dest`.
                //
                // The map is keyed by `(id, clamped physical size)`, so the same
                // program drawn at two sizes in one frame resolves each quad to
                // its own texture — recompute the identical key the pre-pass
                // stored the entry under (`physical_size` then `clamp_size` with
                // the same `adapter_max`).
                let (w, h) = crate::shader_effects::clamp_size(
                    crate::renderer::physical_size(*transform, *dest),
                    adapter_max,
                );
                match shader_images.get(&(program.id(), w, h)) {
                    Some(image) => sink.draw_image(*transform, image, dest),
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
                            *transform,
                            &Brush::Solid(Color::from_rgba8(16, 16, 16, 255)),
                            dest,
                        );
                    }
                }
            }
        }
    }
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
        radius: f64,
    ) {
        // `kurbo::RoundedRect` implements `Shape`, so it fills through the same
        // path as a plain rect (task 41 / RESEARCH.md pinned API).
        let rounded = RoundedRect::from_rect(*rect, radius);
        self.fill(style, transform, brush, None, &rounded);
    }

    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64) {
        let line = Line::new(p0, p1);
        self.stroke(&Stroke::new(width), transform, brush, None, &line);
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun) {
        // `FontHandle` wraps `peniko::FontData`, which is exactly what
        // `vello::Scene::draw_glyphs` accepts in 0.9 (task 03 rationale).
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

    fn pop_clip(&mut self) {
        self.pop_layer();
    }

    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect) {
        // vello's `Scene::draw_image` draws at the image's *natural* pixel
        // size under the given transform (task 57 / RESEARCH.md §Image); map
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
        // punch up to 15 px past an unaligned rect edge (t11-redo defect D4;
        // pixel-proven by `tests/gpu_smoke.rs`'s unaligned-edge probe on Metal
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
    use frust_scene::{FontHandle, Glyph, GlyphRun, SceneBuilder};
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
            radius: f64,
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
            radius: f64,
        ) {
            assert_eq!(style, Fill::NonZero);
            self.events.push(Event::RoundedRect {
                rect: *rect,
                radius,
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
    fn rounded_rect_maps_to_rounded_fill_with_radius() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.fill_rounded_rect(rect, 3.0, Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::RoundedRect {
                rect,
                radius: 3.0,
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
        // feeding that empty map through `encode_scene_with_shaders` (the exact
        // call `SurfaceRenderer::encode`'s Gpu arm makes) must lower every
        // `Command::ShaderQuad` to the placeholder fill, identical to a
        // never-had-a-pre-pass caller.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((3.0, 4.0));
        builder.push_transform(translate);
        let program = frust_scene::ShaderProgram::new("fn main() {}");
        let dest = Rect::new(0.0, 0.0, 40.0, 40.0);
        builder.draw_shader(&program, dest, 1.0);

        // `encode_scene_with_shaders` targets a real `vello::Scene`; drive the
        // same underlying walk (`encode_into_with_shaders`) with a
        // `RecordingSink` instead so the assertion needs no GPU device.
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
        // Review finding M1's encode-level half: when the surface's RESOLVED
        // translucency is `false` (an opaque swapchain — including a
        // `TranslucentPreferred` request that fell back, see context.rs's
        // `forced_mismatch_translucent_request_resolves_not_translucent`), the
        // slot widget emits NO `ClearRect`, so the encode walk must produce no
        // `clear_rect` on the sink at all — nothing gets `DestOut`-zeroed and
        // the slot region simply keeps whatever painted there (Mode A: the
        // native view covers it from on top).
        //
        // Asserted at ENCODE level rather than at the recording-`PaintScene`
        // level deliberately (the t11-redo D4 lesson: a recording-level test
        // missed a real defect in this exact punch path).
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
        // The exact D1 shape: an opaque backdrop fill, then a slot clear over
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
    /// don't panic through the actual `SceneSink` impl — the round-trip
    /// acceptance criterion, not just the `RecordingSink` structural check.
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
        // real `vello::Scene` sink without panicking (the acceptance criterion).
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

        assert_eq!(
            sink.events,
            vec![Event::StrokePath {
                path,
                width: 2.5,
                transform: Affine::IDENTITY,
            }]
        );
    }

    /// A widget can paint a stroked arc via `PaintScene`/`SceneBuilder` without
    /// any `frust-render` dependency, and it reaches a real `vello::Scene`
    /// without panicking — the task 05 acceptance criterion.
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
}
