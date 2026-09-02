# Lab 1 — The Display List (`frust-scene`)

**Concept:** Frust widgets never talk to the GPU. They record *commands* into
a renderer-agnostic display list — `frust_scene::Scene` — which is the stable
seam between the widget world and whatever backend renders it (vello GPU,
vello_cpu, or a test fake). If you understand this crate, you understand the
contract everything else plugs into. It is ~4 small files.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `Scene` — `Vec<Command>` + a reusable `transform_stack: Vec<Affine>` | `crates/frust-scene/src/scene.rs` | ≈179–188 |
| `Command` enum — **all 14 variants** | `crates/frust-scene/src/scene.rs` | ≈33–171 |
| `SceneBuilder` — thin `&mut Scene` wrapper, all record methods | `crates/frust-scene/src/builder.rs` | ≈14–184 |
| `GlyphRun` / `Glyph` / `FontHandle` | `crates/frust-scene/src/glyph.rs` | ≈16–60 |
| `PaintScene` trait (what widgets see) | `crates/frust-core/src/widget.rs` | ≈40–193 |
| `impl PaintScene for SceneBuilder` (the bridge) | `crates/frust-core/src/widget.rs` | ≈203–277 |

The 14 `Command` variants, verbatim:

`FillRect{rect, brush, transform}` · `RoundedRect{rect, radius, brush, transform}` ·
`Line{p0, p1, width, brush, transform}` · `GlyphRun(GlyphRun)` ·
`PushClip{rect, transform}` ·
`PushClipRounded{rect, radius, transform}` — a rounded-corner clip popped by
the same `PopClip` (an avatar/thumbnail mask; no separate rounded clip stack) ·
`PopClip` · `Image{data, dest, transform}` ·
`BlurredRoundedRect{rect, radius, std_dev, color, transform}` ·
`PushLayer{rect, alpha, transform}` · `PopLayer` ·
`ClearRect{rect, transform}` — clears to full transparency, erasing everything
beneath it in the scene (the platform-view hole-punch's sole v1 producer) ·
`Path{path, style, brush, transform}` ·
`ShaderQuad{program, dest, transform, time}` — a fragment-shader-filled rect
(the shader-showcase pre-pass; recognised-but-skipped by the engine's compiler
today — it lowers to a no-op placeholder until the GPU-seam phase wires
`frust_gpu::effects::ShaderEffects` back up, see [RENDER_ARCHITECTURE.md](../RENDER_ARCHITECTURE.md))

That's the entire drawing vocabulary of the framework. Every button, every
page transition, every emoji ends up as a sequence of these.

## The two ideas worth internalizing

1. **Commands are flat; the transform stack is capture-time.** There is no
   scene *graph*. `SceneBuilder::push_transform` pushes an `Affine` onto
   `Scene.transform_stack`; every record method calls `current_transform()`
   (`builder.rs` ≈32) and bakes the *composed* transform into the command it
   pushes. By the time a `Command` exists, the stack is irrelevant to it.
   Read `fill_rect` (`builder.rs` ≈60–67) — it's 8 lines.
2. **The `Scene` is an arena reused across frames.** `SceneBuilder::new`
   resets the stack; the shell calls `Scene::reset()` (`scene.rs` ≈197) each
   frame so the `Vec` allocations survive. Nothing is retained frame-to-frame
   at this layer — it's an immediate-mode log over a retained widget tree.

## Experiments

### 1.1 — Read the smallest test, then run it

`fill_rect_uses_identity_transform_by_default`
(`crates/frust-scene/src/builder.rs` ≈199–219) builds a `Scene`, records one
rect, and asserts the exact `Command`. Then
`transform_stack_composes_for_fill_rect` (≈222–254) does the same under a
two-level transform stack.

```bash
cargo test -p frust-scene
```

### 1.2 — Your own scene playground

Add a scratch test at the bottom of `builder.rs` (or a new
`crates/frust-scene/tests/playground.rs`):

```rust
#[test]
fn playground() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.push_transform(Affine::translate((100.0, 50.0)));
    b.push_clip(Rect::new(0.0, 0.0, 40.0, 40.0));
    b.fill_rect(Rect::new(0.0, 0.0, 80.0, 80.0), Brush::Solid(Color::from_rgb8(255, 0, 0)));
    b.pop_clip();
    b.pop_transform();
    for (i, cmd) in scene.commands().iter().enumerate() {
        println!("{i}: {cmd:?}");
    }
}
```

Run with `cargo test -p frust-scene playground -- --nocapture`. Predict the
transforms baked into each command *before* looking. (Delete the test after —
or keep it; it's your notebook.)

### 1.3 — See a real app's command stream

The fastest way to demystify "what does a frame actually contain": in
`crates/frust-shell-desktop/src/app_handler.rs`, right after the paint pass
fills the scene (search for `RenderRoot::paint` / `scene.reset()` — chapter 3
has exact anchors), temporarily add:

```rust
log::info!("frame commands: {}", self.scene.commands().len());
```

then `FRUST_LOG=info cargo run` in `benchmarks/frust_bench` (S1, the
animation-storm scenario, runs by default). Watch the count: a play-area-
derived bubble field × (gradient fill + stroke + 2 glyph runs) + HUD. Now
pause the sim (Pause button) — does the count change? Why not? (Hint:
painting is not gated on *change* at this layer; skipping is the shells' job
— chapters 3 and 7.)

## What to notice before moving on

- `frust-scene` depends on `kurbo` + `peniko` **only** — grep its
  `Cargo.toml`: no vello, no wgpu. That's the scene-layer purity rule from
  `docs/ARCHITECTURE.md`, and it's what makes chapter 4's backend swap
  (GPU↔CPU) a one-seam affair.
- `GlyphRun` carries *positioned* glyphs (`id, x, y`) — by the time text
  reaches the scene, shaping and layout already happened (chapter 6).
