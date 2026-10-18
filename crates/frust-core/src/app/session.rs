//! Session state on [`RenderRoot`]: pointer capture, focus/IME, hover, cursor and clipboard
//! accessors, plus the change-guarded writers that keep their edge generations honest.

use std::rc::Weak;

use super::{RenderRoot, advance_focus_epoch};
use crate::event::{CursorIcon, ImeState, PointerId};
use crate::overlay::{OverlayHit, OverlayKey};
use crate::view::View;

/// Replace `slot` and bump `generation` only when the value actually moves: paint republishes the
/// IME surface every frame, so an unconditional bump would fire the shell's edge every vsync.
fn store_ime_state_in(slot: &mut Option<ImeState>, generation: &mut u64, next: Option<ImeState>) {
    if *slot != next {
        *slot = next;
        *generation = generation.wrapping_add(1);
    }
}

/// End the focus/IME session, the one primitive every release site goes through: clear
/// `focus_active` and the IME surface and, if either was set, bump `generation` exactly once and
/// advance and republish `focus_epoch`, stranding every focus link recorded against the session.
pub(super) fn release_focus_session_in(
    focus_active: &mut bool,
    ime_slot: &mut Option<ImeState>,
    generation: &mut u64,
    focus_epoch: &mut u64,
    root_identity: u64,
) {
    let moved = *focus_active || ime_slot.is_some();
    *focus_active = false;
    *ime_slot = None;
    if moved {
        *generation = generation.wrapping_add(1);
        // Only on an actual release: a `Down` on already-blurred chrome must move nothing.
        *focus_epoch = advance_focus_epoch(*focus_epoch);
        crate::widget::set_live_focus_session(root_identity, *focus_epoch, *focus_epoch);
    }
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Whether a captured pointer gesture is currently in flight.
    pub fn is_pointer_captured(&self) -> bool {
        self.capture_claimant.is_some()
    }

    /// The contact that claimed the capture in flight (only its `Up`/`Cancel` ends it), or `None`.
    pub fn pointer_capture_claimant(&self) -> Option<PointerId> {
        self.capture_claimant
    }

    /// Whether the capture in flight routes the gesture's other contacts to its captor.
    pub fn pointer_capture_contacts(&self) -> bool {
        self.capture_contacts
    }

    /// Whether some widget in the tree currently holds keyboard/IME focus.
    pub fn is_focus_active(&self) -> bool {
        self.focus_active
    }

    /// Whether the last hover pass left a widget holding the hover link; a rebuild removing the
    /// claimant ends it. A touch drag raises it too (LIMITATIONS `hover-window-leave-standing`).
    pub fn is_hover_active(&self) -> bool {
        self.hover_active
    }

    /// The cursor the tree last asked the host to show: re-resolved on every pointer `Move`
    /// ([`CursorIcon::Default`] when nothing asked) and standing through every other pass.
    /// Residual: a widget that is torn down (or moves out from under a stationary pointer) while
    /// its request stands leaves the last shape in place until the next `Move` re-resolves it.
    pub fn cursor(&self) -> CursorIcon {
        self.cursor
    }

    /// Take (and clear) the text a widget asked the shell to put on the host clipboard; drain after
    /// every [`RenderRoot::event`]. Destructive: a write is an edge, so a dropped result is lost.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.pending_clipboard_write.take()
    }

    /// Take (and clear) whether a widget asked to read the host clipboard; the shell answers with a
    /// new `EditCommand::Paste` dispatch, dropping an async answer once `focus_epoch` has moved.
    pub fn take_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_paste_request)
    }

    /// The IME surface the focused widget published, for the platform input method. `None` is the
    /// only no-session form: an inactive publish is a release, so `is_some()` means a live session.
    pub fn ime_state(&self) -> Option<ImeState> {
        self.ime_state.clone()
    }

    /// Bumped on every actual change of `is_focus_active` or `ime_state` — the frame gate's
    /// `focus_or_ime_changed` edge. It counts surface changes, not sessions (see `focus_epoch`).
    pub fn focus_ime_generation(&self) -> u64 {
        self.focus_ime_gen
    }

    /// The live focus session's identity, advanced once per honoured claim and once per release
    /// (so it moves when focus crosses fields that publish alike). Never `0`: a safe sentinel.
    pub fn focus_epoch(&self) -> u64 {
        self.focus_epoch
    }

    /// Set the focus flag, bumping the generation only on change (in practice it only sets).
    pub(super) fn set_focus_active(&mut self, active: bool) {
        if self.focus_active != active {
            self.focus_active = active;
            self.focus_ime_gen = self.focus_ime_gen.wrapping_add(1);
        }
    }

    /// The `&mut self` form of [`store_ime_state_in`].
    pub(super) fn store_ime_state(&mut self, ime: Option<ImeState>) {
        store_ime_state_in(&mut self.ime_state, &mut self.focus_ime_gen, ime);
    }

    /// The `&mut self` form of [`release_focus_session_in`], also clearing the session's owner
    /// surface, key forward, selection toolbar and keyboard-focus flag. Idempotent.
    pub(super) fn release_focus_session(&mut self) {
        self.set_keyboard_focus(false);
        release_focus_session_in(
            &mut self.focus_active,
            &mut self.ime_state,
            &mut self.focus_ime_gen,
            &mut self.focus_epoch,
            self.root_identity,
        );
        self.focus_surface = None;
        self.key_forward = None;
        // The toolbar belongs to the session: end it on the blurring pass, not a paint later.
        if self.selection_toolbar.take().is_some() {
            self.selection_toolbar_gen = self.selection_toolbar_gen.wrapping_add(1);
        }
    }

    /// Modal focus trap: while a modal pod is registered, end a session no floated pod holds (the
    /// main tree's), so it takes no more keys. Not restored when the modal closes (v1).
    pub(super) fn trap_modal_focus(&mut self) {
        if !self.focus_active || !self.overlay_hits.iter().any(OverlayHit::is_modal) {
            return;
        }
        self.publish_focus_session();
        let floated_holds = self
            .overlay_pods
            .iter()
            .filter_map(Weak::upgrade)
            .any(|pod| pod.try_borrow().map_or(true, |pod| pod.holds_live_focus()));
        if !floated_holds {
            self.release_focus_session();
        }
    }

    /// Record a pass's forward request against the session live after it; none without one.
    pub(super) fn record_key_forward(&mut self, requested: Option<OverlayKey>) {
        if let Some(key) = requested.filter(|_| self.focus_active) {
            self.key_forward = Some((key, self.focus_epoch));
        }
    }

    /// The forward target, while the session it was recorded under is still the live one.
    pub(super) fn live_key_forward(&self) -> Option<OverlayKey> {
        self.key_forward
            .filter(|&(_, epoch)| self.focus_active && epoch == self.focus_epoch)
            .map(|(key, _)| key)
    }

    /// Publish this root's at-rest focus session for pods to compare stamps against; called at the
    /// head of every pass, since the thread-local channel mirrors one root at a time.
    pub(super) fn publish_focus_session(&self) {
        crate::widget::set_live_focus_session(
            self.root_identity,
            self.focus_epoch,
            self.focus_epoch,
        );
    }

    /// End the standing hover link outside a hover pass: advance the epoch (stranding every stamp
    /// at once) and clear the mirror.
    pub(super) fn end_hover_link(&mut self) {
        self.hover_epoch = self.hover_epoch.wrapping_add(1);
        self.hover_active = false;
        self.hover_link = None;
        crate::event::set_live_hover_link(self.root_identity, 0);
    }
}
