# Lab 3 — Anatomy of a Frame (desktop shell)

**Concept:** One function in `frust-shell-desktop` runs the entire pipeline
per frame, and with `FRUST_TRACE=1` the framework itself narrates the five
passes. Your job in this lab: hold the code and the trace output side by
side until every log field maps to a line you've read.

## The frame, with anchors

All in `crates/frust-shell-desktop/src/app_handler.rs` (method `window_event`
starts ≈758):

| Step | Code | Anchor |
|---|---|---|
| Entry | `WindowEvent::RedrawRequested =>` | ≈895 |
| **Rebuild** | `runtime.with_owner(\|\| scope.track(\|\| root.rebuild(app_logic, state)))` | ≈938 |
| **Layout** | `root.layout_with_text(logical_size, text_ctx)` | ≈962 |
| Scene clear | `scene.reset()` — reuses last frame's allocations | ≈977 |
| Clock | `FrameTime::from_nanos(self.epoch.elapsed()...)` | ≈982 |
| **Paint** | `root.paint(...)` inside a `SceneBuilder` scope | ≈983–990 |
| Animation continuation | `paint_outcome.needs_frame` → `request_redraw()` | ≈999 |
| Mid-frame signal write | `scope.is_dirty()` → `request_redraw()` | ≈1009 |
| **Encode** | `SurfaceRenderer::encode()` | ≈1028 |
| **Present** | `SurfaceRenderer::present()` (only if `Encoded`) | ≈1048 |
| Record timings | `frame_stats.record(FramePasses {..})` | ≈1062–1069 |

Three things make this loop what it is:

1. **`app_logic` runs *inside* `scope.track(..)`** — every signal `.get()`
   during rebuild subscribes it. A later `.set()` anywhere (including a tokio
   timer on another thread) fires the `FrameWaker` →
   `ShellUserEvent::SignalsDirty` → `request_redraw()`. That's the entire
   reactive story: no diffing framework, just "re-run tracked rebuild."
2. **The desktop shell is dirty-driven.** `winit` `ControlFlow::Wait` +
   redraws requested only on input, `needs_frame`, or signal wakes → idle CPU
   ≈ 0. (Mobile inverts this: continuous vsync callbacks + a skip gate —
   chapter 7.)
3. **The five passes are timed individually** into `FramePasses { rebuild,
   layout, paint, encode, present }` (`crates/frust-shell-common/src/perf.rs`
   ≈150–171) — which is exactly what the trace prints.

## Experiments

### 3.1 — Match the trace to the code

```bash
cd benchmarks/frust_bench && FRUST_TRACE=1 cargo run
```

You'll get one startup line, then periodic (~2s) summaries:

```
frust-perf startup ... first_rebuild_done=.. first_encode_done=.. first_frame_presented=..
frust-perf frame n=.. total_p50_ms=.. rebuild_p95_ms=.. layout_p95_ms=.. paint_p95_ms=.. encode_p95_ms=.. present_p95_ms=.. over_60hz=.. skipped=..
```

For each field, find the `Instant::now()` pair that produced it
(`app_handler.rs` ≈932/939 rebuild, ≈961/963 layout, ≈983/991 paint,
≈1027/1031 encode, ≈1046/1055 present). Questions to answer from your own
numbers:

- Where does the S1 animation storm spend its frame — paint (CPU command
  recording) or encode (vello translate + GPU submit)? Which did you *expect*?
- Why is `present_p95` often the biggest number, and why is that *not* a
  problem? (It's the vsync/swapchain wait, not work — the encode/present
  split exists precisely so you don't misread blocking-on-vsync as cost.)

### 3.2 — Prove the wake path with zero input

The huddle example has timer-driven state. Run `(cd examples/huddle && cargo
run)`, take your hands off mouse and keyboard, and confirm content still
updates. Then find the chain that made it happen, in order:
`FrameWaker` (`crates/frust-reactive/`) → `ShellUserEvent::SignalsDirty` →
`pump_local` + `request_redraw` (search both symbols in
`app_handler.rs`). This is `docs/DEVELOPMENT.md`'s manual visual gate, now
understood rather than just checked.

### 3.3 — Watch rebuild get cheaper than you think

In the S1 scenario, the physics runs in *paint* (the chart widget), but the
HUD label rebuilds only when the FPS signal writes (~1×/sec). Add a
`log::info!` counter inside `BenchApp::build`
(`benchmarks/frust_bench/src/lib.rs`) and one inside the chart's `paint`.
Compare rates while idle vs. while
dragging. You're watching the view/widget split do its job: cheap descriptors
re-made per frame only when *something* wants a frame, retained widgets doing
the heavy lifting.

### 3.4 — (Optional, destructive) What `scene.reset()` protects you from

vello's own docs (chapter 5 tells you where they live on disk) warn that an
un-reset retained scene grows until it crashes the host. Don't actually ship
this — but commenting the `scene.reset()` call and watching the S1 scenario's
encode times climb frame-over-frame is a memorable way to learn what the
display list *is*. Revert immediately.

## What to notice before moving on

- The rebuild/layout/paint passes never touch wgpu, and encode/present never
  touch widgets. The `Scene` you studied in chapter 1 is the *only* thing
  crossing that line — next chapter crosses it with you.
