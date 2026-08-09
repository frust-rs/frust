//! Input delivery: the touch path (raw or resampler-buffered), the mobile
//! IME state-sync pair, and [`under_root_owner`] — the reactive-owner wrap
//! every event pass in this shell runs under.

use frust_core::event::{
    EditingState, ImeState, InputEvent, PointerButton, PointerEvent, PointerPhase,
};
use frust_reactive::ReactiveRuntime;
use frust_shell_common::resample::RawPointerSample;
use kurbo::Point;

use crate::ffi_support::TouchPhase;

use super::IosAppHandle;

/// Run one **event pass** under the reactive runtime's root
/// [`Owner`](frust_reactive::Owner), so `use_context` resolves from inside a
/// touch/IME handler exactly as it does from `Component::build`.
///
/// Every path that reaches [`AppTree::event`](frust_shell_common::AppTree::event)
/// — touch dispatch (raw and frame-resampled), `ime_apply`, and a queued
/// accessibility action — routes through this. Without it `Owner::current()` is
/// `None` for the whole pass (`Owner::with` restores the previous owner when the
/// rebuild wrap returns), so a handler's `use_context::<Theme>()` silently
/// resolves to `None`.
///
/// Three deliberate properties, mirrored in the Android shell and pinned by the
/// desktop shell's `event_pass_*` tests (the mobile `app` modules are
/// target-gated and never host-compiled, so that is where this shape is
/// testable):
///
/// * **The root owner, not a fresh child scope.** A child owner would have to be
///   created and disposed per input event — including per resampled `Move` —
///   and a handler's `provide_context` would evaporate on dispose. Sharing costs
///   one thread-local swap per pass and no allocation.
/// * **No [`TrackedScope`](frust_reactive::TrackedScope).** `TrackedScope::track`
///   clears the scope's recorded
///   sources and dirty flag on entry, so tracking an event pass would unsubscribe
///   the frame loop from every signal the last rebuild read *and* swallow a
///   pending wake. A handler that writes a signal still wakes the shell through
///   the rebuild scope's own subscription, unchanged.
/// * **Never panics.** Like the rebuild wrap, it degrades to running the pass
///   unwrapped if the runtime is somehow absent — this path is reached across the
///   C-ABI boundary, where an unwind is undefined behavior.
pub(super) fn under_root_owner<R>(pass: impl FnOnce() -> R) -> R {
    match ReactiveRuntime::get() {
        Some(rt) => rt.with_owner(pass),
        None => pass(),
    }
}

impl IosAppHandle {
    /// Deliver one touch contact to the tree.
    ///
    /// **Coordinate asymmetry vs Android:** UIKit's `touch.location(in:)` is
    /// already in **logical points**, so — unlike the Android shell, which
    /// receives physical pixels and divides by the display density — this path
    /// passes `x`/`y` straight through with no scale division. First-touch only
    /// in v1: the Swift side forwards a single contact as
    /// [`PointerButton::Primary`]. The redraw is implicit — the `CADisplayLink`
    /// loop posts a frame every vsync, so the mutated state is picked up on the
    /// next `frame()` without an explicit schedule (contrast the desktop shell's
    /// `request_redraw`).
    pub(crate) fn dispatch_touch(&mut self, phase: TouchPhase, x: f32, y: f32) {
        let position = Point::new(x as f64, y as f64);
        let core_phase = match phase {
            TouchPhase::Began => PointerPhase::Down,
            TouchPhase::Moved => PointerPhase::Move,
            TouchPhase::Ended => PointerPhase::Up,
            TouchPhase::Cancelled => PointerPhase::Cancel,
        };
        // Latch for the frame gate: a touch between frames must force the next
        // frame to run so the mutated state is reflected.
        self.events_since_last_frame = true;

        // Pointer resampling: buffer the raw sample (stamped
        // on the shared resample clock) for [`Self::frame`] to emit a
        // frame-boundary-interpolated position; Down/Up/Cancel still pass
        // through losslessly. When the kill switch disabled the resampler,
        // deliver directly instead — pre-resampling behavior verbatim.
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

    /// Push a whole editing state from the platform IME mirror into the focused
    /// widget (the mobile state-sync path). Delegates to
    /// `AppTree::ime_apply`; the `EditingState`'s selection/composing indices are
    /// UTF-16 code units (converted to byte offsets by the widget/`frust-text`).
    /// The `needs_redraw` in the returned outcome is implicit here — the
    /// `CADisplayLink` loop already ticks the next frame every vsync — so it is
    /// dropped (mirror of [`Self::dispatch_touch`]).
    pub(crate) fn ime_apply(&mut self, state: EditingState) {
        let app = &mut self.app;
        let _ = under_root_owner(|| app.ime_apply(state));
        // Latch for the frame gate: an IME edit between frames must force the
        // next frame to run (mirror of [`Self::dispatch_touch`]).
        self.events_since_last_frame = true;
    }

    /// The IME surface the focused widget published (editing state + caret), for
    /// the FFI layer to serialize back to the Swift `UITextInput` bridge.
    /// Delegates to `AppTree::ime_state`; `None` when nothing is focused.
    pub(crate) fn ime_state(&self) -> Option<ImeState> {
        self.app.ime_state()
    }
}

/// The devtools UI-thread view of this handle (see
/// `frust_shell_common::devtools`). Injection lands on exactly the path
/// `dispatch_touch`/`ime_apply` above use — the `AppTree::event` seam inside
/// [`under_root_owner`], plus the `events_since_last_frame` frame-gate latch —
/// so a synthetic tap is hit-tested and routed like a real one and cannot be
/// skipped by the gate on the frame that follows it. Coordinates are logical
/// points, matching this shell's own touch path (no scale division).
#[cfg(feature = "devtools")]
impl frust_shell_common::devtools::DevtoolsUi for IosAppHandle {
    fn inspect(&self) -> Vec<frust_core::InspectNode> {
        self.app.inspect()
    }

    fn dispatch(&mut self, event: InputEvent) {
        self.events_since_last_frame = true;
        let app = &mut self.app;
        let _ = under_root_owner(|| app.event(&event));
    }
}
