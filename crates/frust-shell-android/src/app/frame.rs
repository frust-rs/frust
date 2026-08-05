//! One Choreographer tick: the per-frame input gathering, the frame-gate
//! decision it feeds, and — on a run — the rebuild → conditional layout → paint
//! → submit pass order the whole shell is built around.

use std::any::Any;
use std::time::{Duration, Instant};

use frust_core::FrameTime;
use frust_core::event::InputEvent;
use frust_reactive::ReactiveRuntime;
use frust_scene::SceneBuilder;
use frust_shell_common::perf::{self, UiSpans};
use frust_shell_common::resample;
use frust_shell_common::{FrameInputs, FramePacing, SurfaceSize, logical_size, sanitize_scale};
use kurbo::{Affine, Size};

use crate::sync_tail::TailSignals;

use super::AndroidAppHandle;
use super::input::under_root_owner;

impl AndroidAppHandle {
    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path but driven by Choreographer.
    ///
    /// `frame_time_nanos` is Kotlin's Choreographer `frameTimeNanos` for this
    /// tick (already clamped non-negative at the JNI boundary — see
    /// [`crate::jni_glue::native_on_frame`]), the shell-owned monotonic clock
    /// threaded into [`FrameTime`] (`frust-core` never reads a clock
    /// itself).
    ///
    /// A no-op when the surface isn't `SurfaceReady` (Kotlin keeps posting frames
    /// across surface loss; this makes those cheap). On `FrameOutcome::SurfaceLost`
    /// the machine has already dropped the surface; recovery waits for the next
    /// `surfaceChanged`/`surfaceCreated` rather than recreating mid-frame.
    ///
    /// # Frame gate
    ///
    /// The pass order is contract-critical: **pump first**, then gather every
    /// [`FrameInputs`] signal, then `FrameGate::decide`. On a [`Skip`] the
    /// rebuild/layout/paint/encode/present passes are all bypassed (only a
    /// `skipped` `FramePasses` is recorded); on a [`Run`] the passes proceed
    /// as before, with the layout pass itself finer-gated on the drained
    /// [`ChangeFlags`](frust_core::view::ChangeFlags). Every input either
    /// reads a tree accessor or a handle-side latch cleared here — see the
    /// inline comments at each gather site for what each signal means.
    ///
    /// The pump → gather → decide ordering and the latch lifecycle are asserted
    /// only by inspection, not a unit test: this whole module is
    /// `#[cfg(target_os = "android")]` (it needs a live GPU surface + JNI handle
    /// to construct an [`AndroidAppHandle`]), so it never runs under the host
    /// `cargo test --workspace`. The decision logic it drives *is* exhaustively
    /// host-tested where it lives — `frust_shell_common::frame_gate`'s
    /// `decide`/warmup/kill-switch tests — so what stays unverified here is only
    /// the wiring, which the `cargo check --target aarch64-linux-android` gate
    /// compile-checks and a manual device checklist exercises.
    ///
    /// [`Skip`]: frust_shell_common::FrameDecision::Skip
    /// [`Run`]: frust_shell_common::FrameDecision::Run
    pub(crate) fn frame(&mut self, frame_time_nanos: u64) {
        // ---------------------------------------------------------------
        // Per-tick input gathering (contract order):
        // pump FIRST, then gather every FrameInputs signal, THEN decide.
        // ---------------------------------------------------------------

        // Advance the scroll-sync tail ahead of EVERY early
        // return in this function — the frame gate's skip, the surface-ready
        // bail — because a held geometry batch ages in **display** frames, and
        // the Choreographer keeps delivering those while frust produces
        // nothing. That is what makes the settle case structural: a batch left
        // over from the last frame of a fling lands on schedule even though the
        // scroll produced no further frust frame.
        let acquire_wait_us = self.executor.acquire_wait_us();
        let tail_trace = self.sync_tail.tick(TailSignals {
            frame_time_nanos: frame_time_nanos as i64,
            expected_present_delta_nanos: self.frame_timeline_delta_nanos,
            acquire_wait_us,
        });
        // One line per depth-or-regime *change*, plus a rate-limited periodic
        // snapshot even without one (`ScrollSyncTail::tick`'s
        // `DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES`, so the emit-or-not decision
        // stays host-tested rather than living here): a handful of lines per
        // fling, never per frame, behind the same `perf::enabled()` switch as
        // every other instrumented line here. The periodic fallback is what
        // makes a stock `FRUST_TRACE` profile build answer "why didn't the
        // regime latch" on a new device — a change-only line stays silent for
        // the whole session there. `acquire_us` is the render side's
        // acquire-wait EWMA (the regime discriminator), not a raw sample;
        // `expected_present_us`/`period_us` are the depth's numerator/divisor;
        // `depth`/`target` are the ramped-vs-requested hold. `frame=` is the
        // tail's display-frame counter, which the gesture markers in
        // `dispatch_touch` stamp too: subtracting the two is the onset
        // measurement.
        if let Some(trace) = tail_trace
            && perf::enabled()
        {
            log::info!(
                "frust-perf platform-view tail depth={} target={} acquire_bound={} \
                 frame={} expected_present_us={} acquire_us={acquire_wait_us} period_us={}",
                trace.depth,
                trace.target_depth,
                trace.acquire_bound,
                trace.display_frame,
                self.frame_timeline_delta_nanos / 1000,
                (self.sync_tail.period_ms() * 1000.0) as u64,
            );
        }

        // Pump the reactive runtime's local task queue BEFORE anything else: a
        // controller-driven `spawn_local` task must keep draining every
        // Choreographer tick even while the surface is torn down (e.g.
        // mid-rotation) or not yet created, not just once it's ready — otherwise
        // local tasks stall through surface churn. Pumping first also matters
        // for correctness: a signal a just-drained local task
        // writes must be observed by *this* frame's dirty check below.
        crate::jni_glue::pump_reactive_runtime();

        // Poll the app-facing theme override slot once per
        // frame, before the surface-ready gate — theme delivery needs no
        // renderer, so this stays in sync even while the surface is torn down
        // (mirroring the reactive-runtime pump just above). A poll that changes
        // the theme is a frame-gate input (`theme_or_appearance_changed`); the
        // ladder it resolves through lives beside the rest of the shell's
        // theme ownership (see [`Self::poll_theme_override`]).
        let mut theme_or_appearance_changed = self.poll_theme_override();

        // Poll the app-facing pending-font registry once per frame,
        // beside the theme poll above and before the surface-ready gate — the
        // drain needs no renderer, so it stays in sync through surface churn.
        // `drain_into` applies any late-registered fonts to `text_ctx` (clearing
        // the shape cache internally) and returns whether anything registered.
        // On a late drain, force the relayout `register_fonts` documents by
        // re-pushing the currently-active theme through `set_theme` (via
        // `push_theme` — the same LAYOUT|PAINT contract a theme swap uses, no new
        // core API), so `Text`'s layout-baked shaping re-runs against the new
        // faces; that also feeds the frame gate (`change_flags_pending`, plus the
        // explicit `theme_or_appearance_changed` bit here per the default-to-run
        // rule). When nothing is pending this is one cheap `Mutex` check.
        if self.font_registry.drain_into(&mut self.text_ctx) {
            self.push_theme();
            theme_or_appearance_changed = true;
        }

        // Apply accessibility actions queued by assistive tech since the last
        // frame, before the surface-ready gate and before the
        // rebuild below so an action's state change is reflected this frame.
        // Cheap (a no-op) whenever nothing is queued, which is the common case.
        // Whether anything was applied is a frame-gate input.
        let a11y_action_performed = self.apply_pending_accessibility_actions();

        if !self.executor.has_surface() {
            // Surface not ready (Kotlin keeps posting frames across surface
            // loss): nothing to render or gate. Inline reads the renderer's phase;
            // the split reads its UI-side `surface_active` mirror (the render
            // thread owns the real phase — a scene handed off while the render
            // thread is still creating the surface is dropped render-side). The
            // reactive pump + theme poll above already ran so state stays live
            // through surface churn; the event/surface latches are intentionally
            // *not* cleared here so the first ready frame still sees them. No
            // FrameStats row is recorded for a not-ready tick (it never was
            // pre-gate either).
            return;
        }

        // Resolved-translucency sync, before the gate
        // inputs are gathered: a render-thread fallback-to-opaque flips
        // `RenderRoot::set_surface_translucent` to `false`, which marks
        // `ChangeFlags::PAINT` and therefore forces THIS frame to run (via
        // `change_flags_pending` below) and repaint without the hole punch.
        // The returned value also drives the base color further down, so the
        // clear color and the punch contract can never disagree.
        let translucent_resolved = self.sync_translucent_resolved();

        // Reactive signals-dirty, drained only past the surface-ready
        // gate — mirroring the iOS shell — so a signal written during a
        // not-ready window is never consumed by a tick that can't render; it is
        // observed by the first ready frame instead. The pump-first ordering
        // contract still holds (the pump above runs before this drain). On a
        // frame the gate goes on to skip the drain is still correct: a skip
        // means "nothing changed", so there is no dirty edge to preserve.
        let signals_dirty = ReactiveRuntime::get()
            .map(|rt| rt.take_signals_dirty())
            .unwrap_or(false);

        // Gather the remaining inputs from the tree's existing accessors and the
        // handle-side latches, then let the gate decide. `mem::take` clears each
        // latch as it is read, so a skipped frame does not leave a stale signal
        // for the next tick.
        //
        // `pointer_capture_active` reads the dedicated `AppTree` accessor
        // (`is_pointer_captured`); `focus_or_ime_changed` compares
        // `AppTree::focus_ime_generation` against this handle's cache. Both are
        // the sources the iOS shell's gate uses — the two frame() bodies must
        // stay input-for-input comparable.
        let inputs = FrameInputs {
            signals_dirty,
            // Pending buffered pointer samples (a sample too new for this tick's
            // instant) must keep frames running until drained — the resampler's
            // pending signal ORs into the events input (the
            // "never starves the gate" contract; default-to-run rule).
            events_since_last_frame: std::mem::take(&mut self.events_since_last_frame)
                || self.resampler.has_pending(),
            pointer_capture_active: self.app.is_pointer_captured(),
            // Focus/IME EDGE, drained inline like every other latch here: the
            // live generation replaces the cached one and the comparison IS the
            // input. One Run per focus/IME transition; a steady focus session
            // (a caret blinking in an idle field) reports `false` and leaves the
            // gate free to idle or pace — as a level read this input rendered
            // every Choreographer tick for the whole session (62–120 fps
            // measured on a Xiaomi 12) and made caret pacing unreachable. The
            // platform-side IME reconcile is unaffected: Kotlin's per-frame
            // `doFrame` poll reads the published Rust state directly, whether or
            // not this tick produces a frame.
            focus_or_ime_changed: {
                let generation = self.app.focus_ime_generation();
                std::mem::replace(&mut self.last_focus_ime_gen, generation) != generation
            },
            last_needs_frame: self.last_needs_frame,
            last_needs_frame_paced_only: self.last_needs_frame_paced_only,
            change_flags_pending: self.app.has_pending_change_flags(),
            // A deferred state-bearing callback marked during a state-free pass
            // is owed a `Housekeeping` broadcast only `RenderRoot::rebuild` can
            // dispatch, so the frame that reaches that rebuild must run. A
            // NON-draining peek (the drain is the rebuild's, on a frame that
            // actually runs), mirroring `change_flags_pending` above.
            deferred_callbacks_pending: frust_core::has_pending_result_flush(),
            // The `appearance_dirty` latch (set by `set_appearance`) is taken
            // only past the surface-ready gate — like `signals_dirty` above —
            // so an appearance flip during a not-ready window is observed by
            // the first ready frame instead of being discarded. (`push_theme`'s
            // LAYOUT|PAINT change flags carry correctness either way; the
            // explicit latch is belt-and-suspenders, mirrored on iOS.)
            theme_or_appearance_changed: theme_or_appearance_changed
                || std::mem::take(&mut self.appearance_dirty),
            surface_changed_or_resized: std::mem::take(&mut self.surface_dirty),
            a11y_action_performed,
            // The gate's own warmup counter (seeded by `note_resumed`) drives
            // the resume-warmup Run; leaving this `false` and relying on the
            // counter avoids double-counting (both force a Run identically).
            resumed_recently: false,
        };

        // The surface (re)creation / resize that set `surface_dirty` also forces
        // the layout pass this frame (new dimensions must take effect); ditto the
        // very first frame, before any layout has established geometry.
        let force_layout = inputs.surface_changed_or_resized || !self.first_layout_done;

        // Perf instrumentation: the process-wide
        // switch is one cached bool read (`perf::enabled`'s `OnceLock`), not a
        // clock read — every `Instant::now()` below is gated behind it via
        // `bool::then`, so a disabled build/run never reads a timer on this
        // hot path (guard first, per this module's perf convention).
        let perf_on = perf::enabled();

        // Deadline-aware pacing: estimate this frame's
        // target budget from the tick-to-tick delta, updating the stored tick
        // every frame (skip or run) so the estimate always reflects one refresh
        // interval rather than a gap across skipped ticks.
        let frame_interval = resample::frame_interval_nanos(
            self.last_frame_time_nanos.replace(frame_time_nanos),
            frame_time_nanos,
        );

        // Animation pacing (frame-gate pacing): a frame whose ONLY dirtiness is
        // a paced (CosmeticLoop) request is throttled to the active theme's
        // `cosmetic_loop_rate` rather than reproduced every Choreographer tick.
        // `now` is this tick's Choreographer clock (the same domain `paint`
        // consumes below); the interval is `1 / rate` resolved from the live
        // theme so an app that retunes the token re-paces without a restart.
        // Every other FrameInputs signal still forces an immediate Run — pacing
        // never delays real work (see `frame_gate`'s pacing docs).
        let pacing = FramePacing {
            now: FrameTime::from_nanos(frame_time_nanos),
            interval: Duration::from_secs_f32(1.0 / self.theme.motion.cosmetic_loop_rate.hz()),
        };

        if self.frame_gate.decide_paced(inputs, pacing).is_skip() {
            // Skip path: nothing changed — return before rebuild, so
            // CPU/GPU stay near idle. Inline records a `skipped` FramePasses
            // (all-zero pass durations) so the skip counter accumulates in the
            // perf log line. In the render-thread split a Skip sends **nothing**
            // across the channel (the render thread is the
            // single emitter and never sees skipped frames), so `record_skip` is a
            // no-op there. Either way only frame *production* stops; the
            // Choreographer keeps re-posting callbacks, so the loop cadence is
            // unchanged.
            self.executor.record_skip();
            return;
        }

        // ---------------------------------------------------------------
        // Run path: rebuild -> (layout iff needed) -> paint -> encode/present,
        // timed as before.
        // ---------------------------------------------------------------

        // Pointer resampling: drain buffered samples up to
        // this frame's sample instant and feed the interpolated events into the
        // tree BEFORE the rebuild, so the rebuild reflects this frame's
        // resampled input. Uses the same `resample_clock` domain the raw samples
        // were stamped in. A no-op when the resampler is disabled (touches were
        // delivered directly in `dispatch_touch`). The scratch buffer and the
        // tree are borrowed as disjoint fields so the batch can be iterated
        // in place while dispatching.
        if self.resampler.is_enabled() {
            let now_nanos = self.resample_clock.elapsed().as_nanos() as u64;
            self.pointer_scratch.clear();
            self.resampler
                .resample(now_nanos, &mut self.pointer_scratch);
            // One owner install for the whole drained batch (see
            // [`under_root_owner`]), so the per-frame cost stays flat regardless
            // of how many samples landed.
            let app = &mut self.app;
            let scratch = &self.pointer_scratch;
            under_root_owner(|| {
                for sample in scratch {
                    let _ = app.event(&InputEvent::Pointer(*sample));
                }
            });
        }

        // Rebuild under the root `Owner` AND inside the persistent
        // `TrackedScope` so every signal read this frame subscribes the scope:
        // a later write to any of them trips `signals_dirty` (drained above into
        // `FrameInputs::signals_dirty`), so the frame gate runs the frame that
        // paints the change. Without the `scope.track` wrap a completed async
        // load's write would notify no subscriber and the gate would skip until a
        // touch forced a `Run` (a device-only "stuck on
        // loading" stall). Mirrors the desktop shell
        // (`app_handler.rs` `scope.track` site) and `create_handle`'s initial
        // construction; fields are borrowed disjointly so the tracking closure
        // captures only what the rebuild needs, not all of `self`. Degrade
        // gracefully to an unwrapped rebuild if the runtime is somehow absent —
        // the frame path must never panic across the JNI boundary.
        let rebuild_start = perf_on.then(Instant::now);
        match ReactiveRuntime::get() {
            Some(rt) => {
                let scope = &self.scope;
                let app = &mut self.app;
                rt.with_owner(|| scope.track(|| app.rebuild()));
            }
            None => self.app.rebuild(),
        }
        let rebuild_time = rebuild_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Prompt teardown retire: the rebuild just above
        // is where a removed `platform_view` widget's `View::teardown` runs and
        // reports its slot id. Drain those and dispose each native view right
        // now, instead of waiting out the differ's ~30-frame missing-streak
        // heuristic (which cannot tell a torn-down slot from a culled one). A
        // merely culled slot reports nothing here, so the streak still covers
        // it — that asymmetry is the camera keep-alive contract. Emitted before
        // this frame's `ingest` below, so the Dispose leads the batch; it is a
        // lifecycle command, deliberately NOT paired with a frame (like
        // `suspend_all`), so it releases immediately.
        for slot_id in self.app.take_retired_platform_views() {
            self.platform_view_state.retire(slot_id);
        }

        // Layout-skip seam (see the frame_gate module docs): drain the change
        // flags the rebuild (or a prior `set_theme`) accumulated, and run layout
        // only if they need it — or the first frame / a surface resize forces it.
        // The `set_theme => LAYOUT|PAINT` contract keeps `Text`'s layout-baked
        // glyph color correct across a bare theme swap (it marks LAYOUT pending,
        // so a theme change always relayouts even with no view change). Paint
        // still always runs below, replaying the last-baked geometry on a
        // layout-skipped frame.
        let needs_layout = self.app.take_change_flags().needs_layout();
        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted JNI `jfloat` density
        // must never let the two passes disagree — see `sanitize_scale`).
        let scale = sanitize_scale(self.scale);
        let layout_start = perf_on.then(Instant::now);
        if needs_layout || force_layout {
            let (lw, lh) = logical_size(self.physical.0, self.physical.1, scale);
            let logical = Size::new(lw, lh);
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            self.app.layout(logical, text_ctx);
            self.first_layout_done = true;
        }
        let layout_time = layout_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Publish the accessibility tree post-layout, so node
        // bounds are valid. A cheap no-op unless the tree changed AND a screen
        // reader is active (double-gated inside).
        self.publish_semantics();

        // Push the render side's presented-frame count so a widget measuring FPS
        // reports the presented rate, not its Choreographer paint cadence. A pure
        // observation — `set_presented_frames` marks no ChangeFlags,
        // so a ticking counter never dirties layout NOR feeds the frame gate (the
        // gate decision already ran above and never reads this), keeping the
        // menu-idle behavior intact.
        self.app
            .set_presented_frames(self.executor.presented_frames());

        self.scene.reset();
        let paint_start = perf_on.then(Instant::now);
        {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI: lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (time enters from the shell, never
            // `Instant::now()` inside `frust-core`) — Choreographer's
            // `frameTimeNanos`, forwarded from Kotlin via `nativeOnFrame`.
            let frame_time = FrameTime::from_nanos(frame_time_nanos);
            // The paint pass returns a `needs_frame` continuation signal, the
            // framework's animation seam. The Choreographer keeps posting frames, but
            // the frame gate now decides whether each is *produced* —
            // so this flag is no longer irrelevant: latch it into
            // `last_needs_frame` so an in-flight animation/transition forces the
            // next frame to run (and stops forcing once it settles).
            let outcome = self.app.paint(&mut builder, frame_time);
            self.last_needs_frame = outcome.needs_frame;
            // Latch the aggregated tick-class so the NEXT frame's gate can pace a
            // paced-only decorative loop (see `FrameInputs::last_needs_frame_paced_only`).
            self.last_needs_frame_paced_only = outcome.needs_frame_paced_only;
            builder.pop_transform();
        }
        let paint_time = paint_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Ingest this RUN frame's published platform-view frames into the
        // differ, right after paint — the source paint just
        // populated. Never reached on a Skip (this whole block is behind the
        // gate's early `return` above), so the differ's skip-safety contract
        // (a rect can't "move" during a skip) holds by construction. There is
        // no "push to Kotlin now" path — `nativePlatformViewCommands` is a poll
        // Kotlin drives from its own per-frame callback, mirroring
        // `nativeImeState`/`nativeSystemUiState`.
        //
        // When the differ produced something, pair that batch with the frame
        // that painted it — the one submitted just below, i.e. the submission
        // cursor plus one — so the release gate holds the batch until that
        // frame is on screen. Both statements sit behind the
        // gate's early `return`, so a Skip records nothing AND submits nothing:
        // the recorded id can never run ahead of what will actually be sent.
        if self
            .platform_view_state
            .ingest(self.app.platform_view_frames(), self.app.input_shields())
        {
            let (generation, _) = self.platform_view_state.commands();
            self.platform_view_due
                .record(generation, self.executor.submitted_frame_id() + 1);
        }

        // Hand the finished frame to the render-path executor.
        // The inline fallback runs the encode→acquire→submit tail synchronously
        // here (via the shared `render_scene`) and returns its encode span; the
        // split moves the painted scene out (replacing `self.scene` with a fresh
        // empty one) into a `SceneFrame` and hands it across the channel for the
        // render thread to encode/present, returning `Duration::ZERO` (encode is
        // off-thread). Either way `render_scene` is the single place the folded
        // `FramePasses` is recorded, the first-encode/first-frame startup
        // milestones are stamped, and SurfaceLost/Redraw are handled — the exact
        // pre-split tail, only relocated. The clear color (the live theme's
        // surface color, not white) rides *with* the scene so a mid-frame theme
        // flip clears correctly.
        let ui = UiSpans {
            rebuild: rebuild_time,
            layout: layout_time,
            paint: paint_time,
            skipped: false,
        };
        // Platform-views translucent mode: a
        // surface that RESOLVED translucent (`translucent_resolved`, read at
        // the top of this frame — not the request latch) must clear to alpha-0,
        // not the theme's opaque surface color, so a native sibling view placed
        // behind it shows through wherever this frame painted nothing (Mode B —
        // see `docs/ARCHITECTURE.md`'s Platform-view flow). Opaque —
        // requested-but-unavailable included: bit-for-bit today's behavior.
        let base_color = crate::ffi_support::base_clear_color(
            translucent_resolved,
            peniko::Color::TRANSPARENT,
            self.theme.scheme().surface,
        );
        let size = SurfaceSize {
            width: self.physical.0,
            height: self.physical.1,
            scale: self.scale as f64,
        };
        let frame_time = FrameTime::from_nanos(frame_time_nanos);
        let encode_time =
            self.executor
                .submit_frame(&mut self.scene, base_color, ui, frame_time, size, perf_on);

        // Deadline-aware pacing overrun: this frame's *work*
        // (everything but the vsync `present` wait, which is expected to block)
        // overrunning the tick-to-tick budget is counted and logged. Gated
        // behind `perf_on` so a non-perf build reads no clocks and logs nothing;
        // instrumentation only — no work is dropped on the strength of this. In
        // the render-thread split `encode_time` is zero (encode is off-thread), so
        // `work` reduces to the UI thread's real budget — rebuild+layout+paint —
        // which is exactly what the UI thread is now responsible for hitting.
        if perf_on {
            let work = rebuild_time + layout_time + paint_time + encode_time;
            if resample::deadline_overrun(work, frame_interval) {
                self.deadline_overruns += 1;
                log::info!(
                    "frust-perf deadline overrun_work_us={} budget_us={} total_overruns={}",
                    work.as_micros(),
                    frame_interval / 1_000,
                    self.deadline_overruns,
                );
            }
        }
    }
}
