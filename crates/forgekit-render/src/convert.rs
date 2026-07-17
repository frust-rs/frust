//! Translation of the renderer-agnostic [`forgekit_scene::Scene`] display list
//! into `vello` draw calls.
//!
//! The mapping logic lives behind the [`SceneSink`] trait so it can be unit
//! tested without a GPU (see the tests below, which record calls into a plain
//! `Vec`). The only production implementor is `vello::Scene`; the public
//! [`encode_scene`] entry point is the one place a `vello` type appears in this
//! crate's API — deliberately, so a shell that owns its own `vello::Renderer`
//! can reuse ForgeKit's scene encoding (mirrors how `forgekit-scene` allows
//! `peniko` types, spec §7).

use forgekit_scene::{Command, GlyphRun, Scene};
use kurbo::{Affine, Line, Point, Rect, RoundedRect, Stroke};
use peniko::{Brush, Color, Fill, ImageData};

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
}

/// Encodes every command in `scene` into `target` (a reused `vello::Scene`).
///
/// Call `target.reset()` before this to clear the previous frame — the render
/// path does exactly that (spec §7: rebuild the scene per frame, never
/// accumulate).
pub fn encode_scene(scene: &Scene, target: &mut vello::Scene) {
    encode_into(scene, target);
}

/// Generic worker behind [`encode_scene`]; kept separate so tests can drive it
/// with a non-`vello` sink.
pub(crate) fn encode_into(scene: &Scene, sink: &mut impl SceneSink) {
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
            Command::PushClip { rect, transform } => sink.push_clip(*transform, rect),
            Command::PopClip => sink.pop_clip(),
            Command::Image {
                data,
                dest,
                transform,
            } => sink.draw_image(*transform, data, dest),
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
            } => sink.push_layer(*transform, rect, *alpha),
            Command::PopLayer => sink.pop_layer(),
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
    use forgekit_scene::{FontHandle, Glyph, GlyphRun, SceneBuilder};
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
    }

    fn empty_font() -> FontHandle {
        FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0))
    }

    fn two_by_two_image() -> ImageData {
        ImageData {
            data: Blob::from(vec![0u8; 2 * 2 * 4]),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 2,
            height: 2,
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
    fn clip_and_layer_nest_preserving_push_pop_order() {
        // push_clip -> push_layer -> pop_layer -> pop_clip: the encode step
        // must preserve command-stream order, mirroring the builder-level
        // nesting test in forgekit-scene.
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
