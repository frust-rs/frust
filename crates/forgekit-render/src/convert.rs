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
use peniko::{Brush, Fill};

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
    }

    fn empty_font() -> FontHandle {
        FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0))
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
}
