# Lab 1 — The Display List (`frust-scene`)

**Concept:** Frust widgets never talk to the GPU. They record *commands* into
a renderer-agnostic display list — `frust_scene::Scene` — which is the stable
seam between the widget world and whatever backend renders it: `frust-engine`
on wgpu in production, the CPU oracle in tests (`crates/frust-testing`), or a
recording fake. If you understand this crate, you understand the contract
everything else plugs into. It is ~4 small files.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `Scene` — `Vec<Command>` + a reusable `transform_stack: Vec<Affine>` | `crates/frust-scene/src/scene.rs` | ≈362–380 |
| `Command` enum — **17 variants** | `crates/frust-scene/src/scene.rs` | ≈159–354 |
| `SceneBuilder` — thin `&mut Scene` wrapper, all record methods | `crates/frust-scene/src/builder.rs` | ≈15–334 |
| `GlyphRun` / `Glyph` / `FontHandle` | `crates/frust-scene/src/glyph.rs` | ≈16–60 |
| `PaintScene` trait (what widgets see) | `crates/frust-core/src/widget.rs` | ≈50 |
| `impl PaintScene for SceneBuilder` (the bridge) | `crates/frust-core/src/widget.rs` | ≈400 |

The 17 `Command` variants, verbatim, in enum order:

`FillRect{rect, brush, transform}` — fill an axis-aligned rectangle with a
brush, under a transform ·
`RoundedRect{rect, radii, brush, transform}` — fill an axis-aligned rectangle
with rounded corners ·
`Line{p0, p1, width, brush, transform}` — stroke a straight line segment from
`p0` to `p1` ·
`GlyphRun(GlyphRun)` — draw a positioned run of glyphs ·
`PushClip{rect, transform}` — push a rectangular clip onto the render
backend's clip stack, under a transform; subsequent draws are clipped to it
until the matching `PopClip` ·
`PushClipRounded{rect, radii, transform}` — a rounded-corner clip popped by
the same `PopClip` (an avatar/thumbnail mask; no separate rounded clip stack) ·
`PopClip` — pop the most recently pushed clip ·
`Image{data, dest, transform}` — draw a decoded image, scaled to fill `dest`,
under a transform ·
`BlurredRoundedRect{rect, radii, std_dev, color, transform}` — draw a rounded
rectangle with a gaussian-blurred elevation shadow (a CSS `box-shadow`
approximation), under a transform ·
`PushLayer{rect, alpha, transform}` — push a translucent layer onto the
render backend's layer stack, under a transform; subsequent draws are
composited at `alpha` until the matching `PopLayer` ·
`PopLayer` — pop the most recently pushed layer ·
`ClearRect{rect, transform}` — clears to full transparency, erasing everything
beneath it in the scene (the platform-view hole-punch's sole v1 producer) ·
`Path{path, style, brush, transform}` — fill or stroke an arbitrary vector
path (e.g. an arc), under a transform ·
`ShaderQuad{program, dest, transform, time}` — a fragment-shader-filled rect
(the shader-showcase pre-pass). It lowers through
`crates/frust-engine/src/compile/mod.rs` — see `note_shader_quad_culled`
(≈2060) for debug-level logging when the pre-pass deliberately culled it (the
quad is off-screen), and `note_shader_quad_unrendered` (≈2020) for once-per-
process warning when the pre-pass never ran or the shader failed to compile.
A resolved draw is always composited blended, never claimed opaque — see
[14-engine-paints-images-filters.md](14-engine-paints-images-filters.md) ·
`PushSnapshot{key, rect, alpha, scale, transform}` — marks the start of a
cacheable "snapshot" bracket; `alpha`/`scale` are presentation parameters
applied to the whole bracketed body, not baked into its own commands ·
`PopSnapshot` — pop the most recently pushed snapshot bracket ·
`SceneTexture{id, dest, transform}` — draw an externally owned GPU texture,
scaled to fill `dest`, under a transform; lowers through
`crates/frust-engine/src/compile/external.rs`. An unregistered `id` draws
nothing — see `note_unregistered` (≈218) for once-per-id warning (first
sighting) then debug-level logging (later sightings)

That's the entire drawing vocabulary of the framework. Every button, every
page transition, every emoji ends up as a sequence of these.

## The two ideas worth internalizing

1. **Commands are flat; the transform stack is capture-time.** There is no
   scene *graph*. `SceneBuilder::push_transform` pushes an `Affine` onto
   `Scene.transform_stack`; every record method calls `current_transform()`
   (`builder.rs` ≈33) and bakes the *composed* transform into the command it
   pushes. By the time a `Command` exists, the stack is irrelevant to it.
   Read `fill_rect` (`builder.rs` ≈61–68) — it's 8 lines.
2. **The `Scene` is an arena reused across frames.** `SceneBuilder::new`
   resets the stack; the shell calls `Scene::reset()` (`scene.rs` ≈389) each
   frame so the `Vec` allocations survive. Nothing is retained frame-to-frame
   at this layer — it's an immediate-mode log over a retained widget tree.

## Experiments

### 1.1 — Read the smallest test, then run it

`fill_rect_uses_identity_transform_by_default`
(`crates/frust-scene/src/builder.rs` ≈347–368) builds a `Scene`, records one
rect, and asserts the exact `Command`. Then
`transform_stack_composes_for_fill_rect` (≈370–403) does the same under a
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
  `Cargo.toml`: no wgpu, no `frust-engine`. That's the scene-layer purity
  rule from `docs/ARCHITECTURE.md`, and it's what makes chapter 4's backend
  swap (GPU↔CPU) a one-seam affair.
- `GlyphRun` carries *positioned* glyphs (`id, x, y`) — by the time text
  reaches the scene, shaping and layout already happened (chapter 6).
