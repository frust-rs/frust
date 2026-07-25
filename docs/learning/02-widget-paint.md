# Lab 2 — Paint Your Own Pixels (the widget seam)

**Concept:** A widget is the thing that *emits* chapter 1's commands. The
whole contract is four methods on one trait, and this repo already contains
graded examples from trivial (`Icon`) to full custom canvas (the S1
bubble-chart scenario's chart widget). This lab makes you a producer, not a
reader.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `Widget` trait — `layout`/`paint`/`event`/`semantics` | `crates/frust-core/src/widget.rs` | ≈796–834 |
| `RecordingScene` — GPU-free paint-assertion fake | `crates/frust-core/src/widget.rs` | ≈1162–1174 |
| Smallest real paint impl: `Icon` (scaled `BezPath` fill) | `crates/frust-widgets/src/icon.rs` | ≈298–311 |
| Animation-driven repaint: `LoadingIndicator` | `crates/frust-widgets/src/material/loading_indicator.rs` | ≈160, 194, 205 |
| Full custom canvas: the S1 bubble chart | `benchmarks/frust_bench/src/scenarios/s1_animation/chart.rs` | whole file |
| Custom widgets in an *app* (escape hatch) | `examples/huddle/src/ui/{fill_box,swipeable,sheet,toast}.rs` | see `examples/huddle/Cargo.toml` ≈47–61 |

The signatures you implement:

```rust
fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size;
fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene);
fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult;  // default: Ignored
fn semantics(&self, ctx: &mut SemanticsCtx);                                 // default: no-op
```

Layout is Flutter's model: constraints flow down, your chosen `Size` flows
up. Paint receives the `PaintScene` you met in chapter 1.

## The animation contract (learn it once, here)

`LoadingIndicator::paint` (≈194) calls a private `step(ctx.frame_time())`,
which calls `self.timer.advance(now)` (≈160) on its `AnimationController`;
while the animation is live it calls `ctx.request_frame()` (≈205). That's the
entire scheme:

- **No timers.** Time enters only as `PaintCtx::frame_time()` (shell-fed;
  `Instant::now()` is banned in core/widgets — `docs/CODE_STANDARDS.md`).
- **Repaint is requested, not assumed:** `request_frame()` bubbles up as
  `PaintOutcome::needs_frame`, and the shell schedules exactly one more frame.
  Forget it and your animation freezes the moment input stops — try it below.

## Experiments

### 2.1 — Break the S1 bubble chart, on purpose

`benchmarks/frust_bench/src/scenarios/s1_animation/chart.rs` is a physics
canvas painting a play-area-derived field of radial-gradient circles +
strokes + two text runs each, every frame. Warm up:

```bash
cd benchmarks/frust_bench && cargo run
```

Then edit `chart.rs` and re-run after each change:

1. Find the bubble fill and swap the radial gradient for a solid
   `Brush::Solid` — how does the FPS readout move? (You just measured
   gradient cost at the vello fine-raster stage.)
2. Multiply the bubble count (see `physics.rs`, seed/count constants). At
   what count does your machine drop below 60? Below 30?
3. Find where the widget requests the next frame and comment it out. The
   simulation freezes — until you wiggle the mouse. Explain why using the
   frame-loop vocabulary (chapter 3: dirty-driven desktop shell).

### 2.2 — Assert paint output with zero GPU

`RecordingScene` (`crates/frust-core/src/widget.rs` ≈1162) implements
`PaintScene` by pushing `(origin, size)` / `(origin, text)` tuples into
`Vec`s. Find an existing test using it (search `RecordingScene` in
`widget.rs`'s test module), then write one against any `frust-widgets`
widget: build it, `layout` it with tight constraints, `paint` it into a
`RecordingScene`, assert what got recorded. This is test tier T1
(`docs/TESTING.md`) and it runs in milliseconds.

### 2.3 — Write a widget from scratch

Huddle documents the exact pattern for app-local custom widgets — read the
escape-hatch comment in `examples/huddle/Cargo.toml` (≈47–61), then pick the
smallest model: `examples/huddle/src/ui/fill_box.rs` (a rounded-rect
`View`/`Widget` pair in ~150 lines).

Build a `Sparkline` widget in `frust_bench` or huddle:

- `layout`: return `bc.constrain(Size::new(120.0, 32.0))`.
- `paint`: keep a `VecDeque<f64>` of the last N FPS samples (the S1 scenario
  already computes FPS — feed it through a prop), then `stroke_path` a
  `kurbo::BezPath` polyline through them.
- Bonus: animate new samples in with an `AnimationController` +
  `request_frame`, per the contract above.

You now know: constraints, sizing, path building, brushes, the repaint
contract — i.e., everything chapter 1's commands needed a producer for.

## What to notice before moving on

- `Icon::paint` is 14 lines and half of them are the `Affine::scale` from
  design-box to layout size. Real widgets are mostly *geometry math*, not
  API ceremony.
- Nothing you wrote touched vello or wgpu. Chapter 4 shows where your
  commands cross that line.
