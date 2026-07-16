//! [`SceneBuilder`]: the widget-facing API for recording paint commands into a [`Scene`].

use kurbo::{Affine, Point, Rect};
use peniko::{Brush, Color, ImageData};

use crate::glyph::GlyphRun;
use crate::scene::{Command, Scene};

/// Records paint commands into a [`Scene`], maintaining a transform stack.
///
/// Widgets call [`SceneBuilder::push_transform`]/[`SceneBuilder::pop_transform`]
/// around their subtree's paint calls to compose local offsets/scales with the
/// ancestor transform, mirroring nested widget placement.
pub struct SceneBuilder<'a> {
    scene: &'a mut Scene,
    transform_stack: Vec<Affine>,
}

impl<'a> SceneBuilder<'a> {
    /// Wraps `scene` for recording, starting with an identity transform.
    pub fn new(scene: &'a mut Scene) -> Self {
        Self {
            scene,
            transform_stack: vec![Affine::IDENTITY],
        }
    }

    /// The transform currently in effect (composition of all pushed transforms).
    pub fn current_transform(&self) -> Affine {
        *self
            .transform_stack
            .last()
            .expect("transform stack is never empty")
    }

    /// Pushes a transform, composed with the current one, onto the stack.
    ///
    /// Subsequent paint calls use the composed transform until the matching
    /// [`SceneBuilder::pop_transform`].
    pub fn push_transform(&mut self, transform: Affine) {
        let composed = self.current_transform() * transform;
        self.transform_stack.push(composed);
    }

    /// Pops the most recently pushed transform, restoring the previous one.
    ///
    /// A no-op if called without a matching `push_transform` (the base identity
    /// transform is never popped).
    pub fn pop_transform(&mut self) {
        if self.transform_stack.len() > 1 {
            self.transform_stack.pop();
        }
    }

    /// Records a filled rectangle under the current transform.
    pub fn fill_rect(&mut self, rect: Rect, brush: Brush) {
        let transform = self.current_transform();
        self.scene.push(Command::FillRect {
            rect,
            brush,
            transform,
        });
    }

    /// Records a filled rounded rectangle (uniform corner `radius`) under the
    /// current transform.
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f64, brush: Brush) {
        let transform = self.current_transform();
        self.scene.push(Command::RoundedRect {
            rect,
            radius,
            brush,
            transform,
        });
    }

    /// Records a stroked line segment from `p0` to `p1` under the current
    /// transform.
    pub fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, brush: Brush) {
        let transform = self.current_transform();
        self.scene.push(Command::Line {
            p0,
            p1,
            width,
            brush,
            transform,
        });
    }

    /// Records a glyph run, composing its own transform with the current one.
    pub fn draw_glyph_run(&mut self, mut glyph_run: GlyphRun) {
        glyph_run.transform = self.current_transform() * glyph_run.transform;
        self.scene.push(Command::GlyphRun(glyph_run));
    }

    /// Pushes a rectangular clip onto the render backend's clip stack, recording
    /// the current transform so the clip is applied in the same space as the
    /// draws it encloses.
    pub fn push_clip(&mut self, rect: Rect) {
        let transform = self.current_transform();
        self.scene.push(Command::PushClip { rect, transform });
    }

    /// Pops the most recently pushed clip.
    pub fn pop_clip(&mut self) {
        self.scene.push(Command::PopClip);
    }

    /// Records a decoded image, scaled to fill `dest`, under the current
    /// transform.
    ///
    /// `data` is cloned into the command — cheap, since `ImageData`'s
    /// `Blob<u8>` is reference-counted internally (see [`Command::Image`]).
    pub fn draw_image(&mut self, data: &ImageData, dest: Rect) {
        let transform = self.current_transform();
        self.scene.push(Command::Image {
            data: data.clone(),
            dest,
            transform,
        });
    }

    /// Records a gaussian-blurred rounded-rectangle shadow under the current
    /// transform (see [`Command::BlurredRoundedRect`]).
    pub fn draw_blurred_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f64,
        std_dev: f64,
        color: Color,
    ) {
        let transform = self.current_transform();
        self.scene.push(Command::BlurredRoundedRect {
            rect,
            radius,
            std_dev,
            color,
            transform,
        });
    }

    /// Pushes a translucent layer onto the render backend's layer stack,
    /// recording the current transform (see [`Command::PushLayer`]).
    pub fn push_layer(&mut self, rect: Rect, alpha: f32) {
        let transform = self.current_transform();
        self.scene.push(Command::PushLayer {
            rect,
            alpha,
            transform,
        });
    }

    /// Pops the most recently pushed layer.
    pub fn pop_layer(&mut self) {
        self.scene.push(Command::PopLayer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::{FontHandle, Glyph};
    use peniko::FontData;
    use peniko::color::palette::css::RED;

    fn red_brush() -> Brush {
        Brush::Solid(RED)
    }

    #[test]
    fn fill_rect_uses_identity_transform_by_default() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);

        let rect = Rect::new(0.0, 0.0, 10.0, 20.0);
        builder.fill_rect(rect, red_brush());

        let commands = scene.commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            Command::FillRect {
                rect: got_rect,
                transform,
                ..
            } => {
                assert_eq!(*got_rect, rect);
                assert_eq!(*transform, Affine::IDENTITY);
            }
            other => panic!("expected FillRect, got {other:?}"),
        }
    }

    #[test]
    fn transform_stack_composes_for_fill_rect() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);

        let translate = Affine::translate((10.0, 5.0));
        let scale = Affine::scale(2.0);

        builder.push_transform(translate);
        builder.push_transform(scale);

        let rect = Rect::new(0.0, 0.0, 1.0, 1.0);
        builder.fill_rect(rect, red_brush());

        // Popping one level should leave only `translate` in effect.
        builder.pop_transform();
        builder.fill_rect(rect, red_brush());

        // One more pop returns to the base identity transform; a further pop
        // beyond that is a no-op (the base identity is never popped).
        builder.pop_transform();
        builder.pop_transform();
        builder.fill_rect(rect, red_brush());

        let expected = [translate * scale, translate, Affine::IDENTITY];
        let commands = scene.commands();
        assert_eq!(commands.len(), expected.len());
        for (command, expected_transform) in commands.iter().zip(expected) {
            match command {
                Command::FillRect { transform, .. } => assert_eq!(*transform, expected_transform),
                other => panic!("expected FillRect, got {other:?}"),
            }
        }
    }

    #[test]
    fn glyph_run_round_trips_its_fields() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);

        let font = FontHandle::new(FontData::new(peniko::Blob::from(Vec::<u8>::new()), 0));
        let glyphs = vec![
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
        ];
        let run = GlyphRun {
            font: font.clone(),
            font_size: 16.0,
            brush: red_brush(),
            transform: Affine::IDENTITY,
            glyphs: glyphs.clone(),
        };

        builder.draw_glyph_run(run);

        match &scene.commands()[0] {
            Command::GlyphRun(got) => {
                assert_eq!(got.font_size, 16.0);
                assert_eq!(got.glyphs, glyphs);
                assert_eq!(got.transform, Affine::IDENTITY);
            }
            other => panic!("expected GlyphRun, got {other:?}"),
        }
    }

    #[test]
    fn glyph_run_composes_with_current_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);

        let translate = Affine::translate((3.0, 4.0));
        builder.push_transform(translate);

        let font = FontHandle::new(FontData::new(peniko::Blob::from(Vec::<u8>::new()), 0));
        let run = GlyphRun {
            font,
            font_size: 12.0,
            brush: red_brush(),
            transform: Affine::IDENTITY,
            glyphs: vec![],
        };
        builder.draw_glyph_run(run);

        match &scene.commands()[0] {
            Command::GlyphRun(got) => assert_eq!(got.transform, translate),
            other => panic!("expected GlyphRun, got {other:?}"),
        }
    }

    #[test]
    fn push_pop_clip_emit_commands() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);

        let rect = Rect::new(0.0, 0.0, 5.0, 5.0);
        builder.push_clip(rect);
        builder.pop_clip();

        let commands = scene.commands();
        assert_eq!(commands.len(), 2);
        assert!(matches!(commands[0], Command::PushClip { .. }));
        assert!(matches!(commands[1], Command::PopClip));
    }

    #[test]
    fn rounded_rect_round_trips_radius_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((2.0, 3.0));
        builder.push_transform(translate);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.fill_rounded_rect(rect, 4.0, red_brush());

        match &scene.commands()[0] {
            Command::RoundedRect {
                rect: got,
                radius,
                transform,
                ..
            } => {
                assert_eq!(*got, rect);
                assert_eq!(*radius, 4.0);
                assert_eq!(*transform, translate);
            }
            other => panic!("expected RoundedRect, got {other:?}"),
        }
    }

    #[test]
    fn stroke_line_round_trips_endpoints_and_width() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let p0 = Point::new(1.0, 2.0);
        let p1 = Point::new(9.0, 12.0);
        builder.stroke_line(p0, p1, 2.5, red_brush());

        match &scene.commands()[0] {
            Command::Line {
                p0: g0,
                p1: g1,
                width,
                transform,
                ..
            } => {
                assert_eq!(*g0, p0);
                assert_eq!(*g1, p1);
                assert_eq!(*width, 2.5);
                assert_eq!(*transform, Affine::IDENTITY);
            }
            other => panic!("expected Line, got {other:?}"),
        }
    }

    #[test]
    fn push_clip_captures_current_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let scale = Affine::scale(2.0);
        builder.push_transform(scale);
        builder.push_clip(Rect::new(0.0, 0.0, 5.0, 5.0));

        match &scene.commands()[0] {
            Command::PushClip { transform, .. } => assert_eq!(*transform, scale),
            other => panic!("expected PushClip, got {other:?}"),
        }
    }

    fn two_by_two_image() -> ImageData {
        ImageData {
            data: peniko::Blob::from(vec![0u8; 2 * 2 * 4]),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn draw_image_round_trips_data_and_dest_under_identity_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let data = two_by_two_image();
        let dest = Rect::new(0.0, 0.0, 20.0, 20.0);
        builder.draw_image(&data, dest);

        match &scene.commands()[0] {
            Command::Image {
                data: got_data,
                dest: got_dest,
                transform,
            } => {
                assert_eq!(*got_data, data);
                assert_eq!(*got_dest, dest);
                assert_eq!(*transform, Affine::IDENTITY);
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn draw_image_composes_with_current_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((5.0, 6.0));
        builder.push_transform(translate);
        let data = two_by_two_image();
        builder.draw_image(&data, Rect::new(0.0, 0.0, 4.0, 4.0));

        match &scene.commands()[0] {
            Command::Image { transform, .. } => assert_eq!(*transform, translate),
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn draw_blurred_rounded_rect_round_trips_fields_under_identity_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.draw_blurred_rounded_rect(rect, 4.0, 2.5, RED);

        match &scene.commands()[0] {
            Command::BlurredRoundedRect {
                rect: got_rect,
                radius,
                std_dev,
                color,
                transform,
            } => {
                assert_eq!(*got_rect, rect);
                assert_eq!(*radius, 4.0);
                assert_eq!(*std_dev, 2.5);
                assert_eq!(*color, RED);
                assert_eq!(*transform, Affine::IDENTITY);
            }
            other => panic!("expected BlurredRoundedRect, got {other:?}"),
        }
    }

    #[test]
    fn draw_blurred_rounded_rect_composes_with_current_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((1.0, 2.0));
        builder.push_transform(translate);
        builder.draw_blurred_rounded_rect(Rect::new(0.0, 0.0, 5.0, 5.0), 2.0, 1.0, RED);

        match &scene.commands()[0] {
            Command::BlurredRoundedRect { transform, .. } => assert_eq!(*transform, translate),
            other => panic!("expected BlurredRoundedRect, got {other:?}"),
        }
    }

    #[test]
    fn push_pop_layer_emit_commands_with_alpha_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let scale = Affine::scale(2.0);
        builder.push_transform(scale);
        let rect = Rect::new(0.0, 0.0, 5.0, 5.0);
        builder.push_layer(rect, 0.5);
        builder.pop_layer();

        let commands = scene.commands();
        assert_eq!(commands.len(), 2);
        match &commands[0] {
            Command::PushLayer {
                rect: got_rect,
                alpha,
                transform,
            } => {
                assert_eq!(*got_rect, rect);
                assert_eq!(*alpha, 0.5);
                assert_eq!(*transform, scale);
            }
            other => panic!("expected PushLayer, got {other:?}"),
        }
        assert!(matches!(commands[1], Command::PopLayer));
    }

    #[test]
    fn nested_clip_and_layer_preserve_push_pop_ordering() {
        // push_clip -> push_layer -> pop_layer -> pop_clip: alpha layers must
        // nest correctly with clips, preserving command-stream order.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip_rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let layer_rect = Rect::new(10.0, 10.0, 50.0, 50.0);

        builder.push_clip(clip_rect);
        builder.push_layer(layer_rect, 0.75);
        builder.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), red_brush());
        builder.pop_layer();
        builder.pop_clip();

        let commands = scene.commands();
        assert_eq!(commands.len(), 5);
        assert!(matches!(commands[0], Command::PushClip { .. }));
        assert!(matches!(commands[1], Command::PushLayer { .. }));
        assert!(matches!(commands[2], Command::FillRect { .. }));
        assert!(matches!(commands[3], Command::PopLayer));
        assert!(matches!(commands[4], Command::PopClip));
    }
}
