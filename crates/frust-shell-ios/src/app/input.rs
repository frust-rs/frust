//! Input delivery: the touch path (raw or resampler-buffered), the mobile
//! IME state-sync pair, the clipboard/system-edit-menu channel, and
//! [`under_root_owner`] — the reactive-owner wrap every event pass in this
//! shell runs under.

use frust_core::event::{
    EditCommand, EditingState, ImeState, InputEvent, PointerButton, PointerEvent, PointerId,
    PointerPhase,
};
use frust_core::selection_toolbar::SelectionToolbarRequest;
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
    /// [`PointerButton::Primary`] on [`PointerId::touch`]`(0)`, delivered as an
    /// [`InputEvent::PointerContact`]. The redraw is implicit — the `CADisplayLink`
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
        //
        // Every contact is `touch(0)` here: the platform side forwards one
        // contact, and it is the gesture's first. It travels as an identified
        // `PointerContact` so the root's multi-contact contract (and the
        // resampler's per-contact lane) apply to it.
        let pointer_id = PointerId::touch(0);
        if self.resampler.is_enabled() {
            let time_nanos = self.resample_clock.elapsed().as_nanos() as u64;
            self.resampler.push(RawPointerSample {
                pointer_id,
                phase: core_phase,
                position,
                button: PointerButton::Primary,
                time_nanos,
            });
        } else {
            let event = InputEvent::PointerContact {
                pointer_id,
                event: PointerEvent {
                    phase: core_phase,
                    position,
                    button: PointerButton::Primary,
                },
            };
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

    /// Deliver one clipboard/selection verb the host's edit menu (or a
    /// hardware-keyboard chord UIKit resolved through its responder chain)
    /// asked for, as an `InputEvent::EditCommand` — focus-routed, so it reaches
    /// the focused editable or nobody.
    ///
    /// The sibling of [`Self::dispatch_touch`]/[`Self::ime_apply`] and built
    /// the same way: under [`under_root_owner`] so a handler's `use_context`
    /// resolves, and latching the frame gate so the edit is painted on the next
    /// tick rather than waiting for an unrelated wake. The redraw is implicit —
    /// the `CADisplayLink` loop posts a frame every vsync.
    ///
    /// Never resampled: unlike a pointer move this is a discrete, lossless
    /// command, and a `Paste` carries text no interpolation could ever mean
    /// anything about.
    pub(crate) fn edit_command(&mut self, cmd: EditCommand) {
        let event = InputEvent::EditCommand(cmd);
        let app = &mut self.app;
        let _ = under_root_owner(|| app.event(&event));
        self.events_since_last_frame = true;
    }

    /// Take (and clear) the text a widget asked to put on the host pasteboard —
    /// a focused field's answer to a `Copy`/`Cut`. Delegates to
    /// `AppTree::take_clipboard_write`.
    ///
    /// **Destructive**, like the seam it delegates to: a clipboard write is an
    /// edge, so a caller that drains and drops the result loses it. The Swift
    /// side drains this once per `CADisplayLink` tick and writes any `Some`
    /// straight into `UIPasteboard.general`.
    pub(crate) fn take_clipboard_write(&mut self) -> Option<String> {
        self.app.take_clipboard_write()
    }

    /// Take (and clear) whether a widget asked the shell to read the host
    /// pasteboard back to it. Delegates to `AppTree::take_paste_request`.
    ///
    /// **Destructive**, for [`Self::take_clipboard_write`]'s reason. See
    /// [`crate::ffi_glue::take_paste_request`] for why this direction is
    /// unreachable in practice on iOS, and why it is still wired.
    pub(crate) fn take_paste_request(&mut self) -> bool {
        self.app.take_paste_request()
    }

    /// The selection-toolbar request the focused field published during the
    /// most recent paint — the anchor and enabled-verb set the Swift side
    /// presents the **system** edit menu from. Delegates to
    /// `AppTree::selection_toolbar`; `None` when no field has a selection worth
    /// a menu.
    ///
    /// A **level**, not an edge (contrast the two drains above): it is
    /// republished by every paint the selection survives, so reading it twice
    /// reads the same request. Pair it with
    /// [`Self::selection_toolbar_generation`] to tell "the same standing
    /// selection" from "a new one".
    pub(crate) fn selection_toolbar(&self) -> Option<SelectionToolbarRequest> {
        self.app.selection_toolbar()
    }

    /// The counter that moves on every **actual** change of
    /// [`Self::selection_toolbar`], its clearing included. Delegates to
    /// `AppTree::selection_toolbar_generation`.
    pub(crate) fn selection_toolbar_generation(&self) -> u64 {
        self.app.selection_toolbar_generation()
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
