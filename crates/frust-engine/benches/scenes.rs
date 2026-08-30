//! Scene fixtures shared by frust-engine's diagnostic microbenches.
//!
//! Each builder returns a freshly recorded `frust_scene::Scene`. A bench
//! builds the scene once and reuses it across iterations, mirroring the
//! steady-state shape a real frame compiles: the display list is retained,
//! only compilation happens per frame.

use kurbo::{Affine, Point, Rect, Shape};
use peniko::{Blob, Brush, Color, FontData};

use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};

/// The viewport every fixture is recorded (and compiled) against.
pub const VIEWPORT: (u16, u16) = (1024, 768);

/// A page of `count` small filled rectangles tiled across the viewport — the
/// flatten/tile-heavy shape a scrollable list or grid paints.
pub fn rect_page(count: usize) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    let columns = 20usize;
    for i in 0..count {
        let col = (i % columns) as f64;
        let row = (i / columns) as f64;
        let x0 = col * 48.0;
        let y0 = row * 32.0;
        builder.fill_rect(
            Rect::new(x0, y0, x0 + 40.0, y0 + 24.0),
            Brush::Solid(Color::from_rgba8(30, 120, 220, 255)),
        );
    }

    scene
}

/// A set of `count` filled circular paths — a vector-drawn icon set, as
/// distinct from a glyph run.
pub fn icon_set(count: usize) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    let columns = 25usize;
    for i in 0..count {
        let col = (i % columns) as f64;
        let row = (i / columns) as f64;
        let center = Point::new(20.0 + col * 36.0, 20.0 + row * 36.0);
        let circle = kurbo::Circle::new(center, 12.0);
        builder.fill_path(
            circle.to_path(0.1),
            Brush::Solid(Color::from_rgba8(240, 240, 240, 255)),
        );
    }

    scene
}

/// A single glyph run of `count` glyphs, placed at identity.
///
/// A placeholder: `SceneCompiler` recognizes `Command::GlyphRun` and skips
/// it (glyph compilation is not wired up yet), so compiling this scene only
/// measures the walk's cost of recognizing and stepping over the command —
/// never real shaping or rasterization. Reserved for the group that will
/// exercise the real path once glyphs compile.
pub fn glyph_run_placeholder(count: usize) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    let font = FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0));
    let glyphs = (0..count)
        .map(|i| Glyph {
            id: i as u32,
            x: i as f32 * 10.0,
            y: 0.0,
        })
        .collect();

    builder.draw_glyph_run(GlyphRun {
        font,
        font_size: 16.0,
        brush: Brush::Solid(Color::from_rgba8(0, 0, 0, 255)),
        transform: Affine::IDENTITY,
        glyphs,
    });

    scene
}
