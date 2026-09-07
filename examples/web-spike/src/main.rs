//! Frust web-shell Phase 0 spike: `wasm32-unknown-unknown` compile probe of
//! the render-engine graph — `frust-scene`, `frust-text`, `frust-gpu`,
//! `frust-engine`, `frust-render`, all reached by path dependency (see
//! `Cargo.toml`). See `RESULTS.md` for the exact commands, flags/cfgs, and
//! findings this probe produced.
//!
//! Deliberately render-less: `frust-reactive` is excluded from this graph
//! (its `tokio` multi-thread runtime init fails on `wasm32-unknown-unknown`
//! — w0-04 lands that arm), and driving a real `wgpu::Adapter`/
//! `RenderContext` end to end needs a browser canvas, which is w0-03's job,
//! not this one. `main` below only walks a representative slice of each
//! crate's public API (`Scene`/`SceneBuilder` from `frust-scene`,
//! `TextContext`/`TextStyle`/`TextLayout` from `frust-text`,
//! `SceneCompiler::compile` from `frust-engine`, and
//! `RenderContext`/`SurfaceRenderer` construction from `frust-gpu`/
//! `frust-render`), so `cargo check --target wasm32-unknown-unknown` is a
//! meaningful type-check of real call sites in every one of the five crates,
//! not just five path deps that happen to resolve.

use kurbo::{Affine, Point, Rect};
use peniko::{Brush, Color};

/// Builds a tiny scene through `frust-scene`'s `SceneBuilder`, including one
/// `frust-text` glyph run converted via `TextLayout::to_scene_runs` — the
/// seam `frust-text`'s own crate doc calls out as the renderer-agnostic
/// boundary onto `frust-scene`.
fn build_scene() -> frust_scene::Scene {
    let mut scene = frust_scene::Scene::new();
    {
        let mut builder = frust_scene::SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, 200.0, 100.0),
            Brush::Solid(Color::from_rgb8(0x33, 0x66, 0x99)),
        );

        let mut text_ctx = frust_text::TextContext::new();
        let style = frust_text::TextStyle::new(16.0, Color::BLACK);
        let layout = text_ctx.layout("Frust web spike", &style, None);
        for run in layout.to_scene_runs(Point::new(8.0, 8.0)) {
            builder.draw_glyph_run(run);
        }
    }
    scene
}

/// Compiles the scene into `frust-engine`'s sparse-strip display list.
///
/// `SceneCompiler::compile` is the CPU half of the engine frame path — it
/// produces a [`frust_engine::CompiledFrame`] of strips/draws without
/// touching a `wgpu::Device`, so it is the deepest real call site into
/// `frust-engine` a browser-less probe can reach. Everything past it
/// (`EngineRenderer`, the GPU pipelines) needs a live adapter, i.e. w0-03.
fn compile_frame(scene: &frust_scene::Scene) -> usize {
    let mut compiler = frust_engine::SceneCompiler::new(200, 100);
    match compiler.compile(scene, Affine::IDENTITY, (200, 100)) {
        Ok(frame) => frame.draws().len(),
        Err(err) => {
            eprintln!("web-spike: scene compile failed: {err:?}");
            0
        }
    }
}

/// Constructs a `frust-gpu` device/surface substrate and a `frust-render`
/// surface renderer. Both constructors are synchronous and enumerate no
/// adapter, so this is a pure type/link check, not a live-device run; there
/// is no browser canvas to hand a real surface to outside w0-03.
fn build_gpu_context() -> (frust_gpu::RenderContext, frust_render::SurfaceRenderer) {
    (
        frust_gpu::RenderContext::new(),
        frust_render::SurfaceRenderer::new(),
    )
}

fn main() {
    let scene = build_scene();
    let draws = compile_frame(&scene);
    let (_ctx, _renderer) = build_gpu_context();
    println!(
        "web-spike: built a {}-command scene, compiled to {draws} engine draws",
        scene.commands().len()
    );
}
