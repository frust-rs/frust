//! [`SceneBuilder`]: the widget-facing API for recording paint commands into a [`Scene`].

use kurbo::{Affine, Rect};
use peniko::Brush;

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

    /// Records a glyph run, composing its own transform with the current one.
    pub fn draw_glyph_run(&mut self, mut glyph_run: GlyphRun) {
        glyph_run.transform = self.current_transform() * glyph_run.transform;
        self.scene.push(Command::GlyphRun(glyph_run));
    }

    /// Pushes a rectangular clip onto the render backend's clip stack.
    pub fn push_clip(&mut self, rect: Rect) {
        self.scene.push(Command::PushClip { rect });
    }

    /// Pops the most recently pushed clip.
    pub fn pop_clip(&mut self) {
        self.scene.push(Command::PopClip);
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
}
