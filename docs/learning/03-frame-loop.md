# Lab 3 — Anatomy of a Frame (desktop shell)

**Concept:** `WindowEvent::RedrawRequested` in `frust-shell-desktop` runs the
UI half of every frame — rebuild, layout, paint — then hands the finished
scene to a shared `render_frame` helper that runs the encode→acquire→submit
tail, either inline on the same thread or (the default) on a dedicated render
thread. With `FRUST_TRACE=1` the framework itself narrates all six passes.
Your job in this lab: hold the code and the trace output side by side until
every log field maps to a line you've read.

## The frame, with anchors

The UI-thread passes (rebuild/layout/paint) live in
`crates/frust-shell-desktop/src/app_handler.rs` (method `window_event` starts
≈2528); the encode→acquire→submit tail is a **shared helper**,
`render_frame`, in `crates/frust-shell-desktop/src/render.rs`, called by both
executors (see "The render-thread split" below):

| Step | Code | File · Anchor |
|---|---|---|
| Entry | `WindowEvent::RedrawRequested =>` | `app_handler.rs` ≈2702 |
| **Rebuild** | `runtime.with_owner(\|\| scope.track(\|\| root.rebuild(app_logic, state)))` | `app_handler.rs` ≈2771 |
| **Layout** | `root.layout_with_text(logical, text_ctx)` | `app_handler.rs` ≈2794 |
| Scene clear | `scene.reset()` — reuses last frame's allocations | `app_handler.rs` ≈2809 |
| Clock | `FrameTime::from_nanos(self.epoch.elapsed()...)` | `app_handler.rs` ≈2814 |
| **Paint** | `root.paint(...)` inside a `SceneBuilder` scope | `app_handler.rs` ≈2823–2829 |
| Animation continuation | `decision.request_redraw` → `window.request_redraw()` | `app_handler.rs` ≈2899–2900 |
| Mid-frame signal write | `scope.is_dirty()` → `request_redraw()` | `app_handler.rs` ≈2910–2911 |
| Handoff to executor | `self.executor.submit_frame(&window, &mut self.scene, ...)` | `app_handler.rs` ≈2935 |
| **Encode** | `renderer.encode(render_cx, scene, base_color)` (→ [`SurfaceRenderer::encode`], `renderer.rs` ≈858) | `render.rs` ≈834–835 |
| **Acquire** | `renderer.acquire(render_cx)` (→ [`SurfaceRenderer::acquire`], `renderer.rs` ≈951) | `render.rs` ≈844–846 |
| **Submit** | `renderer.submit(render_cx)` (→ [`SurfaceRenderer::submit`], `renderer.rs` ≈1064) | `render.rs` ≈855–857 |
| Record timings | `frame_stats.record(FramePasses::from_split(ui_spans, RenderSpans {..}))` | `render.rs` ≈880 |

`SurfaceRenderer::present` (`renderer.rs` ≈927) still exists as a combined
acquire+submit convenience wrapper — kept for a caller (a test, or
`SurfaceRenderer::render` ≈831, its own thin `encode`+`present` wrapper) that
wants present timed as one span. The desktop frame loop doesn't call it: it
needs the finer acquire-vs-submit split (below) for the trace's
`acquire_p95_ms`/`submit_p95_ms` fields.

## The render-thread split

`self.executor` (`app_handler.rs`) is a `FrameExecutor` chosen once at
startup by the `FRUST_NO_RENDER_THREAD` kill switch
(`docs/DEVELOPMENT.md`'s Instrumentation table):

- **`Split` (default):** a dedicated `frust-render` thread owns the
  `RenderContext`/`SurfaceRenderer` and runs `render_loop`
  (`render.rs` ≈566), which calls `render_frame` (≈817) whenever a finished
  scene arrives across the UI→render channel.
- **`Inline` (kill switch engaged):** `InlineExecutor::submit_frame`
  (`render.rs` ≈342) calls the *same* `render_frame` (≈349) synchronously on
  the UI thread, right where `RedrawRequested` handed it the scene.

Either way `render_frame` (`render.rs` ≈817) is the one place encode, acquire,
submit, and `frame_stats.record` happen — which is why the table above can
point at one function no matter which executor is active.

Four things make this loop what it is:

1. **`app_logic` runs *inside* `scope.track(..)`** — every signal `.get()`
   during rebuild subscribes it. A later `.set()` anywhere (including a tokio
   timer on another thread) fires the `FrameWaker` →
   `ShellUserEvent::SignalsDirty` → `request_redraw()`. That's the entire
   reactive story: no diffing framework, just "re-run tracked rebuild."
2. **The desktop shell is dirty-driven.** `winit` `ControlFlow::Wait` +
   redraws requested only on input, `needs_frame`, or signal wakes → idle CPU
   ≈ 0. (Mobile inverts this: continuous vsync callbacks + a skip gate —
   chapter 7.)
3. **The six passes are timed individually** into `FramePasses { rebuild,
   layout, paint, encode, acquire, submit }` (`crates/frust-shell-common/src/perf.rs`
   ≈222–250) — which is exactly what the trace prints. `encode`/`acquire`/`submit`
   replaced an older combined `encode`/`present` split precisely so the
   blocking vsync wait (`acquire`) is attributable separately from blit +
   queue-submit work (`submit`).
4. **The UI/render split changes *where* a pass runs, never *what* it
   measures.** `FramePasses::from_split` (`perf.rs` ≈334) just recombines the
   UI thread's `rebuild`/`layout`/`paint` with the render thread's
   `encode`/`acquire`/`submit` into the identical record a single-thread frame
   would have produced — same six fields, same wire format.

## Experiments

### 3.1 — Match the trace to the code

```bash
cd benchmarks/frust_bench && FRUST_TRACE=1 cargo run
```

You'll get one startup line, then periodic (~2s) summaries:

```
frust-perf startup ... first_rebuild_done=.. first_encode_done=.. first_frame_presented=..
frust-perf frame n=.. total_p50_ms=.. total_p95_ms=.. total_p99_ms=.. rebuild_p95_ms=.. layout_p95_ms=.. paint_p95_ms=.. encode_p95_ms=.. acquire_p95_ms=.. submit_p95_ms=.. over_60hz=.. over_120hz=.. skipped=.. total_frames=..
```

For each field, find the `Instant::now()` pair that produced it —
`app_handler.rs` ≈2765/2772 rebuild, ≈2793/2795 layout, ≈2822/2830 paint;
`render.rs` ≈834/836 encode, ≈844/852 acquire, ≈855/863 submit (all three
inside the shared `render_frame`, so the same anchors apply whichever
executor is active). Questions to answer from your own numbers:

- Where does the S1 animation storm spend its frame — paint (CPU command
  recording) or encode (`SceneCompiler::compile` + `EngineRenderer::encode` —
  chapters 11 and 13)? Which did you *expect*?
- Why is `acquire_p95` often the biggest number, and why is that *not* a
  problem? (It's the vsync/swapchain wait, not work — encode/acquire/submit
  are split into three spans precisely so you don't misread blocking-on-vsync
  as cost, and can tell it apart from `submit`'s blit + queue-submit work.)
- Set `FRUST_NO_RENDER_THREAD=1` and re-run. The trace's fields don't change
  shape — same six passes — because `render_frame` is the one function both
  executors call; only *which thread* ran encode/acquire/submit changes.

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

`Scene::reset()` (`crates/frust-scene/src/scene.rs` ≈389) is the only thing
that clears the `Vec<Command>` between frames — skip it and every frame's
commands pile onto the last one's forever, until the process runs out of
memory. Don't actually ship this — but commenting out the `scene.reset()`
call in `app_handler.rs` and watching the S1 scenario's encode times climb
frame-over-frame is a memorable way to learn what the display list *is*.
Revert immediately.

## What to notice before moving on

- The rebuild/layout/paint passes never touch wgpu, and encode/acquire/submit
  never touch widgets. The `Scene` you studied in chapter 1 is the *only*
  thing crossing that line — next chapter crosses it with you.
