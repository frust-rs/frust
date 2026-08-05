//! The per-tick frame body: the `CADisplayLink`-driven
//! rebuild → layout → paint → hand-off loop, and the gathering of every
//! [`FrameInputs`] signal the shared frame gate decides run/skip from.
//!
//! One module on purpose — the gate inputs are only meaningful beside the
//! passes that produce and consume them, and every drain here is ordered
//! against the pause/ready gate (a signal taken on a no-op tick is a lost
//! wake, not a saved one).

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

use super::IosAppHandle;
use super::input::under_root_owner;

impl IosAppHandle {
    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path but driven by the Swift
    /// `CADisplayLink`.
    ///
    /// `timestamp_ns` is the `CADisplayLink` tick's `timestamp`
    /// (`CFTimeInterval` seconds) converted to nanoseconds by the Swift caller
    /// — the shell-owned monotonic clock threaded into [`FrameTime`]
    /// (`frust-core` never reads a clock itself).
    ///
    /// A no-op unless the surface is ready *and* the app is not paused (see
    /// [`crate::ffi_support::should_render_frame`]). Readiness comes from the
    /// executor: inline reads the renderer's phase, the split reads its UI-side
    /// `surface_active` mirror. On `FrameOutcome::SurfaceLost` (surfaced
    /// render-side in the split, or from the inline tail) the surface is dropped;
    /// recovery recreates it from the retained `metal_layer` — render-side on the
    /// same wakeup in the split, or on the next `frust_render_frame`/`frust_resize`
    /// FFI entry in the inline path (see [`crate::ffi_glue`]).
    ///
    /// The render tail is off this thread in the split — the first-presented-frame
    /// startup span is recorded render-side inside
    /// [`render_scene`](super::render_scene) (the render thread is the single perf
    /// emitter), so this method no longer returns anything.
    pub(crate) fn frame(&mut self, timestamp_ns: u64) {
        // Pump the UI-thread reactive local-task queue BEFORE the ready/paused
        // gate below: placed after it, queued `spawn_local` completions (e.g. a
        // signal write scheduled from a background task) would stall for as
        // long as the surface stays not-ready/paused instead of draining as
        // soon as the CADisplayLink ticks. A no-op
        // until `frust_init` has installed the runtime.
        if let Some(rt) = ReactiveRuntime::get() {
            rt.pump_local();
        }

        // Poll the app-facing theme override slot once per
        // frame, before the ready/paused gate — theme delivery needs no
        // renderer, so this stays in sync even while backgrounded/not-ready
        // (mirroring the reactive-runtime pump just above). Whether it changed is
        // also a frame-gate input (`theme_or_appearance_changed`) captured here.
        // The precedence ladder the poll resolves against lives beside its
        // helpers in `super::theme` (`IosAppHandle::poll_theme_override`).
        let mut theme_or_appearance_changed = self.poll_theme_override();

        // Poll the app-facing pending-font registry once per frame,
        // beside the theme poll above and before the pause/ready gate — the drain
        // needs no renderer, so it stays in sync while backgrounded/not-ready.
        // `drain_into` applies any late-registered fonts to `text_ctx` (clearing
        // the shape cache internally) and returns whether anything registered. On
        // a late drain, force the relayout `register_fonts` documents by
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

        // Pause/ready gate FIRST — a paused/not-ready frame does no work and the
        // frame gate is never even consulted (the existing early return
        // stays first). Returning here also leaves the signals-dirty flag
        // undrained (it is only `take`n past this gate below), so a tracked-signal
        // write that lands while backgrounded is observed by the first frame
        // after resume rather than being silently consumed on a no-op tick.
        let ready = self.executor.has_surface();
        if !crate::ffi_support::should_render_frame(ready, self.paused) {
            return;
        }

        // Resolved-translucency sync, before the gate
        // inputs are gathered below: a render-thread fallback-to-opaque (or a
        // self-healed recreate that resolved differently) flips
        // `RenderRoot::set_surface_translucent` to `false`, which marks
        // `ChangeFlags::PAINT` and so forces THIS frame to run
        // (`change_flags_pending`) and repaint without the hole punch. The
        // returned value also drives the base color below, so the clear color
        // and the punch contract can never disagree.
        let translucent_resolved = self.sync_translucent_resolved();

        // Drain any queued accessibility actions (VoiceOver activations, etc.)
        // BEFORE the rebuild below, so a state change an action makes is picked up
        // by this very frame — the same "mutate now, rebuild next" model touch/IME
        // input uses. The handler enqueued these on the
        // main thread; draining takes ownership of the batch so the queue's borrow
        // is dropped before `perform_accessibility_action` re-enters the tree.
        // Whether any action ran is a frame-gate input (`a11y_action_performed`).
        let mut a11y_action_performed = false;
        if let Some(a11y) = self.a11y.as_ref() {
            let actions = a11y.drain_actions();
            a11y_action_performed = !actions.is_empty();
            // An action is routed through synthesized pointer events, landing in
            // the very same handlers a real tap would — so the whole batch runs
            // under the reactive root owner (see [`under_root_owner`]).
            let app = &mut self.app;
            under_root_owner(|| {
                for req in actions {
                    // `target_node.0` is the raw accesskit id the adapter
                    // reported; an unknown node or unmodelled action is a benign
                    // no-op (see `RenderRoot::perform_accessibility_action`).
                    // `needs_redraw` is dropped — the CADisplayLink loop already
                    // ticks the next frame.
                    let _ = app.perform_accessibility_action(req.target_node.0, req.action);
                }
            });
        }

        // Render-side surface self-heal, taken past the
        // ready/paused gate like the drains below so a signal can never be
        // consumed on a no-op tick: the render thread recreated the surface from
        // the retained `CAMetalLayer` on its own (nothing external re-drives
        // creation on iOS — see `SplitExecutor::surface_reinstalled`), so this
        // frame must actually run and repaint the fresh swapchain. Replay every
        // live platform-view slot too, mirroring the inline path's
        // `recover_surface`: the replay supersedes any batch still held by the
        // present-sync release gate and clears the pairing, so geometry paired
        // with the frame that was lost cannot strand now that a resting screen
        // produces no further frames to release it.
        let surface_reinstalled = self.executor.take_surface_reinstalled();
        if surface_reinstalled {
            self.reset_platform_views_for_surface_recreate();
        }

        // Gather the OR-list of "something changed" signals and let
        // the frame gate decide whether this frame runs — mirrors the Android
        // shell's own frame-gate wiring. `signals_dirty` is
        // drained AFTER the pump above (the pump-first ordering contract — see
        // `ReactiveRuntime::take_signals_dirty`) and only now that we are past the
        // ready/paused gate, so a no-op tick never consumes it. The gate honors
        // the `FRUST_NO_FRAME_GATE` kill switch internally (always `Run` when
        // disabled). Correctness over savings: every input defaults toward "run".
        let signals_dirty = ReactiveRuntime::get().is_some_and(|rt| rt.take_signals_dirty());
        // The focus/IME session's EDGE input: the live generation is read here
        // and the cache updated below (beside the events-latch reset — this
        // shell defers its resets past the input assembly), so
        // `focus_or_ime_changed` reports "the session moved since the last
        // gathered tick", never "something is focused". As a level read this
        // forced a frame on every `CADisplayLink` tick for the whole life of a
        // focus session and made caret pacing unreachable (62–120 fps measured
        // on a static focused screen). Swift's per-frame `renderFrame` IME
        // reconcile is unaffected: it polls the published Rust state directly,
        // whether or not this tick produces a frame.
        let focus_ime_gen = self.app.focus_ime_generation();
        let inputs = FrameInputs {
            signals_dirty,
            // Pending buffered pointer samples (too new for this tick's instant)
            // keep frames running until drained — the resampler's pending signal
            // ORs into the events input (the "never starves the
            // gate" contract; default-to-run rule).
            events_since_last_frame: self.events_since_last_frame || self.resampler.has_pending(),
            // A mid-drag gesture reads straight from the retained tree's
            // `RenderRoot` state; the focus/IME input is the generation edge
            // gathered just above. Both mirror the Android shell's gate
            // input-for-input (the two frame() bodies stay comparable).
            pointer_capture_active: self.app.is_pointer_captured(),
            focus_or_ime_changed: focus_ime_gen != self.last_focus_ime_gen,
            last_needs_frame: self.last_needs_frame,
            last_needs_frame_paced_only: self.last_needs_frame_paced_only,
            // Non-draining peek: a skipped frame leaves the flags for the next
            // frame that runs to drain.
            change_flags_pending: self.app.has_pending_change_flags(),
            // A deferred state-bearing callback marked during a state-free pass
            // is owed a `Housekeeping` broadcast only `RenderRoot::rebuild` can
            // dispatch, so the frame that reaches that rebuild must run. Also a
            // NON-draining peek (the drain is the rebuild's job, on a frame that
            // actually runs), mirroring `change_flags_pending` above.
            deferred_callbacks_pending: frust_core::has_pending_result_flush(),
            // The `appearance_dirty` latch (set by `set_appearance`) is taken
            // only past the pause/ready gate — like the signals-dirty drain —
            // mirroring the Android shell input-for-input.
            theme_or_appearance_changed: theme_or_appearance_changed
                || std::mem::take(&mut self.appearance_dirty),
            // A UI-driven surface (re)creation/resize is folded into the gate's
            // resume-warmup via `note_resumed` (see `resize`/`set_surface`/
            // `resume`), so it needs no per-frame latch. What DOES need one is
            // the split's render-side self-heal, which never passes through a
            // UI-thread entry point at all (see the self-heal handling above).
            surface_changed_or_resized: surface_reinstalled,
            a11y_action_performed,
            // Driven by the gate's own warmup countdown (`note_resumed`).
            resumed_recently: false,
        };
        // The events latch has now been read into this frame's decision; reset it
        // so the next frame only sees events that arrive from here on. Same for
        // the focus/IME edge cache: this tick has consumed the transition, so
        // the next one compares against what the tree reports now.
        self.events_since_last_frame = false;
        self.last_focus_ime_gen = focus_ime_gen;

        // Deadline-aware pacing: estimate this frame's target
        // budget from the tick-to-tick delta, updating the stored tick every
        // frame (skip or run) so the estimate reflects one refresh interval
        // rather than a gap across skipped ticks.
        let frame_interval = resample::frame_interval_nanos(
            self.last_frame_time_nanos.replace(timestamp_ns),
            timestamp_ns,
        );

        // Animation pacing (frame-gate pacing): a frame whose ONLY dirtiness is
        // a paced (CosmeticLoop) request is throttled to the active theme's
        // `cosmetic_loop_rate` rather than reproduced every `CADisplayLink`
        // tick. `now` is this tick's display-link clock (the same domain `paint`
        // consumes below); the interval is `1 / rate` resolved from the live
        // theme so a retuned token re-paces live, and `requested_interval` is the
        // previous paint's own MIN-folded request
        // (`PaintCtx::request_frame_paced_at` — a caret blink far slower than the
        // cap), which `FramePacing::effective_interval` resolves against it.
        // Every other FrameInputs signal still forces an immediate Run — pacing
        // never delays real work. Mirrors the Android shell seam-for-seam.
        let pacing = FramePacing {
            now: FrameTime::from_nanos(timestamp_ns),
            interval: Duration::from_secs_f32(1.0 / self.theme.motion.cosmetic_loop_rate.hz()),
            requested_interval: self.last_paced_interval,
        };

        if self.frame_gate.decide_paced(inputs, pacing).is_skip() {
            // Nothing changed: skip rebuild/layout/paint/encode entirely. Inline
            // records a `skipped` FramePasses (ZERO pass durations; counts toward
            // `skipped=` in the perf log line); the render-thread split sends
            // **nothing** across the channel on a skip (the render thread is the
            // single emitter and never sees skipped frames — split mode records no
            // skip frames, a known limitation), so `record_skip` is a no-op
            // there. Either way the CADisplayLink keeps ticking — only frame
            // *production* stops, callbacks don't (the accepted v1 shape, same as
            // Android — see `docs/DEVELOPMENT.md`).
            self.executor.record_skip();
            return;
        }

        // Perf instrumentation: read the cached
        // switch exactly once per frame and gate every `Instant::now()` read
        // below behind it — a disabled build takes zero clock reads on this
        // path, not merely a no-op record (`FrameStats::record` itself is
        // also a no-op when disabled, but the timer reads this guard skips
        // are the actual hot-path cost).
        let perf_on = perf::enabled();

        // Pointer resampling: drain buffered samples up to
        // this frame's sample instant and feed the interpolated events into the
        // tree BEFORE the rebuild, so the rebuild reflects this frame's resampled
        // input (same `resample_clock` domain the raw samples were stamped in). A
        // no-op when disabled (touches went straight through in `dispatch_touch`).
        // The scratch buffer and the tree are borrowed as disjoint fields so the
        // batch can be iterated in place while dispatching.
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
        // [`TrackedScope`](frust_reactive::TrackedScope) so every signal read this
        // frame subscribes the scope:
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
        // the frame path must never panic across the C-ABI boundary.
        let rebuild_start = perf_on.then(Instant::now);
        match ReactiveRuntime::get() {
            Some(rt) => {
                let scope = &self.scope;
                let app = &mut self.app;
                rt.with_owner(|| scope.track(|| app.rebuild()));
            }
            None => self.app.rebuild(),
        }
        let rebuild = rebuild_start.map(|t| t.elapsed()).unwrap_or_default();

        // Prompt teardown retire: the rebuild just above
        // is where a removed `platform_view` widget's `View::teardown` runs and
        // reports its slot id. Drain those and dispose each native view right
        // now, instead of waiting out the differ's ~30-frame missing-streak
        // heuristic (which cannot tell a torn-down slot from a culled one). A
        // merely culled slot reports nothing here, so the streak still covers
        // it — that asymmetry is the camera keep-alive contract. Mirrors the
        // Android shell's placement exactly; the resulting batch is a lifecycle
        // command, deliberately NOT paired with a frame (like `suspend_all`), so
        // the present-sync release gate never holds it.
        for slot_id in self.app.take_retired_platform_views() {
            self.platform_views.retire(slot_id);
        }

        // Change-flag DRAIN — the fix for "the iOS frame
        // gate never idles at rest".
        //
        // `FrameInputs::change_flags_pending` above reads
        // `has_pending_change_flags()`, a deliberately NON-draining peek, so a
        // frame the gate SKIPS leaves the dirtiness for the next frame that runs.
        // Draining is the running frame's job — and this
        // shell never did it: `RenderRoot::pending` is only ever cleared by
        // `take_change_flags`, so from the very first frame on (construction's
        // `push_theme` marks LAYOUT|PAINT, and the first rebuild marks
        // LAYOUT|PAINT again) that input latched `true` forever and every
        // `CADisplayLink` tick therefore decided `Run`. Measured on an iPhone SE
        // before this drain: ~44-59 fps of full pipeline work on a screen where
        // nothing changes, `skipped=0` on every raw line, against Android's zero
        // frames in 60 s on the same page.
        //
        // Android drains at exactly this point in its own frame body, as the
        // input to its layout-skip seam (`take_change_flags().needs_layout()`),
        // which is why its gate does idle. iOS still relayouts every `Run`
        // unconditionally (the intra-frame layout skip is not wired here — see
        // `docs/ARCHITECTURE.md`'s iOS frame pipeline), so the drained flags are
        // deliberately dropped rather than gating the layout call below: this
        // frame runs both passes regardless, so nothing is lost by clearing
        // them. Anything that marks flags between frames (`set_theme`,
        // `set_insets`, `set_surface_translucent`, a rebuild) still forces the
        // next frame to run, exactly as on Android.
        let _drained = self.app.take_change_flags();

        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted `f32` scale from the FFI
        // boundary must never let the two passes disagree — see `sanitize_scale`).
        let scale = sanitize_scale(self.scale);
        let (lw, lh) = logical_size(self.physical.0, self.physical.1, scale);
        let logical = Size::new(lw, lh);
        let layout_start = perf_on.then(Instant::now);
        {
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            self.app.layout(logical, text_ctx);
        }
        let layout = layout_start.map(|t| t.elapsed()).unwrap_or_default();

        // Push the accessibility tree AFTER layout (node bounds come from the
        // post-layout geometry), generation-gated so an unchanged tree
        // is never re-walked or re-pushed (see [`Self::publish_semantics`]).
        // Not folded into either pass's timing above/below — it is a11y-conditional
        // work orthogonal to the rebuild/layout/paint/encode split.
        self.publish_semantics();

        // Push the render side's presented-frame count so a widget measuring FPS
        // reports the presented rate, not its `CADisplayLink` paint cadence (task
        // 10). A pure observation — `set_presented_frames` marks no ChangeFlags,
        // so a ticking counter never dirties layout NOR feeds the frame gate (the
        // gate decision already ran above and never reads this), keeping a
        // menu-idle screen at zero frames.
        self.app
            .set_presented_frames(self.executor.presented_frames());

        let paint_start = perf_on.then(Instant::now);
        self.scene.reset();
        let paint_outcome = {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI: lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (time enters from the shell, never
            // `Instant::now()` inside `frust-core`) — the `CADisplayLink`
            // timestamp forwarded from Swift.
            let frame_time = FrameTime::from_nanos(timestamp_ns);
            let outcome = self.app.paint(&mut builder, frame_time);
            builder.pop_transform();
            outcome
        };
        let paint = paint_start.map(|t| t.elapsed()).unwrap_or_default();

        // Platform-view differ ingest: feed this RUN
        // frame's published frames into the command backlog
        // `frust_platform_view_commands_json` serves. Only ever called on a
        // frame that actually painted (never on the `Skip` `return` above) —
        // `PlatformViewState::ingest`'s skip-safety contract.
        //
        // Under present-sync, pair whatever the differ produced with the frame
        // that painted it — the one submitted just below, i.e. the submission
        // cursor plus one — so the release gate holds that geometry until this
        // thread presents that frame (the same shape as the
        // Android shell's always-on pairing). Both statements sit
        // behind the gate's early `return`, so a Skip records nothing AND
        // submits nothing: the recorded id can never run ahead of what will
        // actually be sent.
        let produced = self
            .platform_views
            .ingest(self.app.platform_view_frames(), self.app.input_shields());
        if produced && self.present_sync {
            let (generation, _) = self.platform_views.commands();
            self.platform_view_due
                .record(generation, self.executor.submitted_frame_id() + 1);
        }

        // Latch this paint's `needs_frame` continuation signal (the v1
        // animation seam) for the NEXT frame's gate: unlike before — when
        // the continuous CADisplayLink loop let this flag be dropped — the gate
        // would now skip the follow-up frame an in-flight animation/transition
        // needs, so it is fed forward via `FrameInputs::last_needs_frame`.
        self.last_needs_frame = paint_outcome.needs_frame;
        // Latch the aggregated tick-class for the NEXT frame's gate so a
        // paced-only decorative loop can be throttled (see
        // `FrameInputs::last_needs_frame_paced_only`), and beside it the
        // MIN-folded interval that loop asked to be paced at
        // (`FramePacing::requested_interval`; `None` = the theme's cap).
        self.last_needs_frame_paced_only = paint_outcome.needs_frame_paced_only;
        self.last_paced_interval = paint_outcome.paced_interval;

        // Hand the finished frame to the render-path executor.
        // The inline fallback runs the encode→acquire→submit tail synchronously
        // here (via the shared [`render_scene`](super::render_scene)) and returns
        // its encode span; the
        // split moves the painted scene out (replacing `self.scene` with a fresh
        // empty one) into a `SceneFrame` and hands it across the channel for the
        // render thread to encode/acquire/present, returning `Duration::ZERO`
        // (encode is off-thread). Either way `render_scene` is the single place
        // the folded `FramePasses` is recorded, the first-encode/first-frame
        // startup milestones are stamped, and SurfaceLost/Redraw are surfaced —
        // the exact pre-split tail, only relocated. The clear color (the live
        // theme's surface color, not white) rides *with* the scene so a mid-frame
        // theme flip clears correctly.
        let ui = UiSpans {
            rebuild,
            layout,
            paint,
            skipped: false,
        };
        // Mode B translucent base clear: a surface that RESOLVED translucent
        // (`translucent_resolved`,
        // read at the top of this frame — not the request latch) clears to
        // alpha-0 instead of the theme's opaque surface color, so a native
        // sibling view placed behind this one shows through wherever the tree
        // paints nothing (Mode B paint contract — the app must paint every
        // chrome surface explicitly). Opaque —
        // requested-but-refused included — is bit-for-bit today's behavior.
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
        let meta_time = FrameTime::from_nanos(timestamp_ns);
        let encode_time =
            self.executor
                .submit_frame(&mut self.scene, base_color, ui, meta_time, size, perf_on);

        // Deadline-aware pacing overrun: this frame's *work*
        // (everything but the vsync `present` wait, which is expected to block)
        // overrunning the tick-to-tick budget is counted and logged. Gated behind
        // `perf_on` so a non-perf build logs nothing; instrumentation only — no
        // work is dropped on the strength of this. In the render-thread split
        // `encode_time` is zero (encode is off-thread), so `work` reduces to the UI
        // thread's real budget — rebuild+layout+paint — which is exactly what the
        // UI thread is now responsible for hitting.
        if perf_on {
            let work = rebuild + layout + paint + encode_time;
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
