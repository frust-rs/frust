//! Input delivery: the JNI touch/IME entry points that run *between* frames,
//! and the reactive-owner wrap every event pass — from here or from the frame
//! loop's resampled drain — runs under.

use frust_core::event::{
    EditingState, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
    PointerEvent, PointerPhase,
};
use frust_reactive::ReactiveRuntime;
use frust_shell_common::perf;
use frust_shell_common::resample::RawPointerSample;
use frust_shell_common::sanitize_scale;
use kurbo::Point;

use crate::ffi_support::TouchPhase;

use super::AndroidAppHandle;

/// Run one **event pass** under the reactive runtime's root
/// [`Owner`](frust_reactive::Owner), so `use_context` resolves from inside a
/// press/key/IME handler exactly as it does from `Component::build`.
///
/// Every path that reaches [`AppTree::event`](frust_shell_common::AppTree::event)
/// — touch dispatch (raw and frame-resampled), `ime_apply`, `ime_action`, and a
/// queued accessibility action — routes through this. Without it
/// `Owner::current()` is `None` for the whole pass (`Owner::with` restores the
/// previous owner when the rebuild wrap returns), so a handler's
/// `use_context::<Theme>()` silently resolves to `None`.
///
/// Three deliberate properties, mirrored in the iOS shell and pinned by the
/// desktop shell's `event_pass_*` tests (the mobile `app` modules are
/// target-gated and never host-compiled, so that is where this shape is
/// testable):
///
/// * **The root owner, not a fresh child scope.** A child owner would have to be
///   created and disposed per input event — including per resampled `Move` —
///   and a handler's `provide_context` would evaporate on dispose. Sharing costs
///   one thread-local swap per pass and no allocation.
/// * **No `TrackedScope`.** `TrackedScope::track` clears the scope's recorded
///   sources and dirty flag on entry, so tracking an event pass would unsubscribe
///   the frame loop from every signal the last rebuild read *and* swallow a
///   pending wake. A handler that writes a signal still wakes the shell through
///   the rebuild scope's own subscription, unchanged.
/// * **Never panics.** Like the rebuild wrap, it degrades to running the pass
///   unwrapped if the runtime is somehow absent — this path is reached from JNI,
///   where an unwind is undefined behavior.
pub(super) fn under_root_owner<R>(pass: impl FnOnce() -> R) -> R {
    match ReactiveRuntime::get() {
        Some(rt) => rt.with_owner(pass),
        None => pass(),
    }
}

impl AndroidAppHandle {
    /// Deliver one touch contact to the tree, converting the incoming
    /// physical view-local coordinates into the logical space the tree lays out
    /// in — the same `sanitize_scale` value `frame()` uses, so hit-testing and
    /// layout never disagree.
    ///
    /// Single-pointer in v1: the Kotlin side forwards only the primary pointer,
    /// so every contact is a [`PointerButton::Primary`] event. The redraw the
    /// tree requests is implicit here — the Choreographer loop already posts a
    /// frame every vsync, so the mutated state is picked up on the next
    /// `frame()` without an explicit schedule (contrast the desktop shell's
    /// `request_redraw`).
    pub(crate) fn dispatch_touch(&mut self, phase: TouchPhase, x: f32, y: f32) {
        let scale = sanitize_scale(self.scale);
        let position = Point::new(x as f64 / scale, y as f64 / scale);
        let core_phase = match phase {
            TouchPhase::Down => PointerPhase::Down,
            TouchPhase::Move => PointerPhase::Move,
            TouchPhase::Up => PointerPhase::Up,
            TouchPhase::Cancel => PointerPhase::Cancel,
        };
        // Frame-gate latch: an event between frames must force the
        // next frame to run so the tree reflects the dispatch. Set even on a
        // no-op dispatch — correctness beats savings, and the gate defaults to
        // "must run" when in doubt.
        self.events_since_last_frame = true;

        // Gesture markers for the scroll-sync onset measurement: the
        // tail's own display-frame counter stamped at the start and
        // end of a gesture, so the frames between the first touch and the hold
        // reaching its depth can be read straight out of a trace instead of
        // eyeballed against logcat wall-clock stamps. Down/Up only (a Move line
        // per frame would drown the trace it serves), and behind the same
        // `perf::enabled()` switch as every other instrumented line here.
        if matches!(core_phase, PointerPhase::Down | PointerPhase::Up) && perf::enabled() {
            log::info!(
                "frust-perf platform-view gesture phase={} frame={}",
                if core_phase == PointerPhase::Down {
                    "down"
                } else {
                    "up"
                },
                self.sync_tail.display_frame(),
            );
        }

        // Pointer resampling: buffer the raw sample (stamped
        // on the shared resample clock) so [`Self::frame`] can emit a
        // frame-boundary-interpolated position; Down/Up/Cancel still pass through
        // losslessly. When the kill switch disabled the resampler, deliver
        // directly instead — pre-resampling behavior verbatim.
        if self.resampler.is_enabled() {
            let time_nanos = self.resample_clock.elapsed().as_nanos() as u64;
            self.resampler.push(RawPointerSample {
                phase: core_phase,
                position,
                button: PointerButton::Primary,
                time_nanos,
            });
        } else {
            let event = InputEvent::Pointer(PointerEvent {
                phase: core_phase,
                position,
                button: PointerButton::Primary,
            });
            let app = &mut self.app;
            let _ = under_root_owner(|| app.event(&event));
        }
    }

    /// Apply a whole editing state pushed by the platform IME (`nativeImeApply`),
    /// routing it to the focused widget as an
    /// [`InputEvent::Ime`]`(`[`ImeEvent::ApplyEditingState`]`)` via
    /// `AppTree::ime_apply`. `state`'s indices are UTF-16 code units (the seam
    /// unit); the focused widget converts them. No explicit redraw is scheduled —
    /// the Choreographer loop already posts the next frame (see [`Self::dispatch_touch`]).
    ///
    /// [`ImeEvent::ApplyEditingState`]: frust_core::event::ImeEvent::ApplyEditingState
    pub(crate) fn ime_apply(&mut self, state: EditingState) {
        // Frame-gate latch: an IME edit between frames forces the next
        // frame to run (see `dispatch_touch`).
        self.events_since_last_frame = true;
        let app = &mut self.app;
        let _ = under_root_owner(|| app.ime_apply(state));
    }

    /// The IME surface the focused widget published, for the FFI layer to
    /// serialise into the `nativeImeState` JSON. Delegates to
    /// `AppTree::ime_state`; `None` when nothing is focused.
    pub(crate) fn ime_state(&self) -> Option<ImeState> {
        self.app.ime_state()
    }

    /// Forward a soft-keyboard editor action (`nativeImeAction`, e.g.
    /// `IME_ACTION_DONE`) as an [`NamedKey::Enter`] key press down the focus path.
    ///
    /// `action` is retained for future differentiation; v1 configures only
    /// `IME_ACTION_DONE`, so every action maps to `Enter`. Reuses the same
    /// focus-routed key path a hardware Enter would, so a widget's
    /// submit/newline handling stays in one place.
    pub(crate) fn ime_action(&mut self, _action: i32) {
        let event = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        // Frame-gate latch: a soft-keyboard action forces the next
        // frame to run (see `dispatch_touch`).
        self.events_since_last_frame = true;
        let app = &mut self.app;
        let _ = under_root_owner(|| app.event(&event));
    }
}

/// The devtools UI-thread view of this handle (see
/// `frust_shell_common::devtools`). Injection lands on exactly the path
/// `dispatch_touch`/`ime_action` above use — the `AppTree::event` seam inside
/// [`under_root_owner`], plus the `events_since_last_frame` frame-gate latch —
/// so a synthetic tap is hit-tested and routed like a real one and cannot be
/// skipped by the gate on the frame that follows it.
#[cfg(feature = "devtools")]
impl frust_shell_common::devtools::DevtoolsUi for AndroidAppHandle {
    fn inspect(&self) -> Vec<frust_core::InspectNode> {
        self.app.inspect()
    }

    fn dispatch(&mut self, event: InputEvent) {
        self.events_since_last_frame = true;
        let app = &mut self.app;
        let _ = under_root_owner(|| app.event(&event));
    }
}
