//! [`EventCtx`], the context a widget's event handler mutates, and the [`EventResult`] /
//! [`EventOutcome`] a dispatch reports.

use std::any::Any;

use kurbo::{Point, Size};

use super::contact::{
    current_pointer_id, note_capture_released, note_contacts_requested, under_contact_captor,
};
use super::ime::ImeState;
use super::key::EditCommand;
use super::pass::{CLIPBOARD_WRITE, CURSOR_REQUEST, EDIT_COMMAND_QUEUE, PASTE_REQUEST};
use super::pointer::{CursorIcon, PointerId};
use crate::overlay::OverlayKey;

/// What a widget did with an event: `Handled` stops the container offering it to further
/// siblings and marks the frame dirty; `Ignored` lets routing continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResult {
    Ignored,
    Handled,
}

/// The result of a whole [`crate::app::RenderRoot::event`] pass: whether any widget consumed the
/// event, and whether the shell should schedule a redraw (the pass itself never repaints).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventOutcome {
    pub handled: bool,
    pub needs_redraw: bool,
}

/// A child dispatch's request flags and published IME surface, folded into its parent by
/// [`EventCtx::absorb_child`].
#[derive(Debug, Default)]
pub(crate) struct ChildFold {
    pub(crate) needs_redraw: bool,
    pub(crate) captured: bool,
    pub(crate) contacts_requested: bool,
    pub(crate) capture_released: bool,
    pub(crate) hover_claimed: bool,
    pub(crate) focus_requested: bool,
    pub(crate) focus_released: bool,
    pub(crate) ime_state: Option<ImeState>,
}

/// Context threaded into [`crate::widget::Widget::event`]: mutate the type-erased app state
/// ([`EventCtx::state_mut`]), request a redraw or layout, capture the pointer or focus, and read
/// the widget's own geometry (event positions are already local).
pub struct EventCtx<'a> {
    state: &'a mut dyn Any,
    needs_redraw: bool,
    needs_layout: bool,
    /// A pod below was marked layout-dirty by this dispatch — the path bit the pod above marks
    /// itself from, also folded into the root's pending `ChangeFlags::LAYOUT`.
    layout_path_dirty: bool,
    capture_requested: bool,
    contacts_requested: bool,
    capture_released: bool,
    pointer_id: PointerId,
    focus_requested: bool,
    focus_released: bool,
    has_focus: bool,
    hovered: bool,
    hover_claimed: bool,
    /// Whether a claim records anything: set only for an uncaptured `Move`, and narrowed per pod so
    /// a captured pointer never hovers and only the first claim of a pass records.
    hover_eligible: bool,
    /// The last completed hover pass's epoch; a claim in this pass records `hover_claim_epoch`.
    hover_epoch: u64,
    hover_root: u64,
    ime_state: Option<ImeState>,
    origin: Point,
    size: Size,
}

impl<'a> EventCtx<'a> {
    /// Build a root event context over the erased `state` for a widget at `origin` with `size`.
    pub fn new(state: &'a mut dyn Any, origin: Point, size: Size) -> Self {
        Self {
            state,
            needs_redraw: false,
            needs_layout: false,
            layout_path_dirty: false,
            capture_requested: false,
            contacts_requested: false,
            capture_released: false,
            // The running root pass's contact, so a context built mid-pass (a component's inner
            // one) reports the same id; the mouse outside any pass.
            pointer_id: current_pointer_id(),
            focus_requested: false,
            focus_released: false,
            has_focus: false,
            hovered: false,
            hover_claimed: false,
            hover_eligible: false,
            hover_epoch: 0,
            hover_root: 0,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Recover the application state as `&mut T`. Panics if `T` is not the type the root erased —
    /// a wiring bug, not a data condition.
    pub fn state_mut<T: Any>(&mut self) -> &mut T {
        self.state
            .downcast_mut::<T>()
            .expect("event state is not the expected application-state type")
    }

    /// Request that the shell schedule a repaint after this event pass.
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Whether a redraw was requested during this (sub)dispatch.
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Request layout for state this widget changed itself: marks every pod up to the root
    /// layout-dirty, reaches the root's pending `ChangeFlags::LAYOUT`, and implies a redraw.
    pub fn request_layout(&mut self) {
        self.needs_layout = true;
        self.needs_redraw = true;
        self.layout_path_dirty = true;
    }

    /// Whether a layout was requested during this (sub)dispatch.
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
    }

    /// Whether this (sub)dispatch dirtied a pod below this context.
    pub(crate) fn is_layout_path_dirty(&self) -> bool {
        self.layout_path_dirty
    }

    /// Fold a child dispatch's layout request and dirty-path bit into this context; also how a
    /// component boundary mirrors its inner context.
    pub(crate) fn absorb_layout(&mut self, child_needs_layout: bool, child_path_dirty: bool) {
        if child_needs_layout {
            self.needs_layout = true;
            self.needs_redraw = true;
        }
        self.layout_path_dirty |= child_path_dirty || child_needs_layout;
    }

    /// Capture the pointer: later moves/releases route back to this widget along the recorded
    /// active path until `Up`/`Cancel`. Only a hit-tested `Down` (or an overlay surface's input)
    /// sets the root's capture mirror, so a capture from a `Move` routes while the root still reads
    /// uncaptured; a drag wanting hover chrome throughout keeps its own pressed flag.
    pub fn capture_pointer(&mut self) {
        self.capture_requested = true;
    }

    /// Whether the widget requested pointer capture during this (sub)dispatch.
    pub fn is_pointer_captured(&self) -> bool {
        self.capture_requested
    }

    /// Which pointer contact this event comes from: [`PointerId::MOUSE`] unless the root unwrapped
    /// an [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact).
    pub fn pointer_id(&self) -> PointerId {
        self.pointer_id
    }

    /// Opt into the gesture's other contacts: call on the `Down` that calls
    /// [`EventCtx::capture_pointer`]; while that capture lives, each other contact arrives here as
    /// a plain pointer event with its own [`EventCtx::pointer_id`]. Another contact's `Up`/`Cancel`
    /// ends nothing; when the claimant's does, drop every other contact being tracked.
    pub fn capture_contacts(&mut self) {
        self.contacts_requested = true;
        note_contacts_requested();
    }

    /// Whether a widget called [`EventCtx::capture_contacts`] during this (sub)dispatch; bubbles
    /// like [`EventCtx::is_pointer_captured`].
    pub fn is_contact_capture_requested(&self) -> bool {
        self.contacts_requested
    }

    /// Take the gesture over: clear `child`'s active path (after sending it `Cancel`). If its
    /// subtree held the contact opt-in, the root stops routing other contacts; the capture stays
    /// with the claimant. A no-op while `child` is inactive, during a non-claimant delivery, or
    /// when the opt-in holder is this container or an ancestor.
    pub fn release_captured_child(&mut self, child: &mut crate::widget::ChildPod) {
        if !child.is_active() {
            return;
        }
        let held_opt_in = child.holds_contact_opt_in();
        child.set_active(false);
        if child.is_active() || !held_opt_in || under_contact_captor() {
            return;
        }
        self.capture_released = true;
        note_capture_released();
    }

    /// Whether [`EventCtx::release_captured_child`] voided the opt-in during this (sub)dispatch.
    pub fn is_capture_released(&self) -> bool {
        self.capture_released
    }

    /// Request focus: the enclosing pods record the focus path, and keyboard, IME and edit-command
    /// events are then delivered down it with no hit test.
    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    /// Release focus: drop the recorded focus path (e.g. Escape / blur).
    pub fn release_focus(&mut self) {
        self.focus_released = true;
    }

    /// Whether the receiving widget holds the focus path (always, for a focus-routed event).
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// From an editable's owner: forward the keys the focused editable declines to the floated
    /// pod `key`, as `OverlayEventKind::Key`, before the shortcut table sees them. Recorded on
    /// the focus session live at the end of this pass; a new claim or a release ends it.
    pub fn forward_declined_keys_to(&mut self, key: OverlayKey) {
        crate::shortcuts::request_key_forward(key);
    }

    /// Consume an `OverlayEventKind::Key` delivery, which stops its walk to the shortcut table:
    /// the delivery is a broadcast, whose `Handled` no container carries back.
    pub fn consume_overlay_key(&mut self) {
        crate::shortcuts::note_handled();
    }

    /// Claim the hover link for this widget and, via the recorded path, its ancestors; the next
    /// paint reports [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered) for them.
    /// Ignored outside an uncaptured `Move` or after the pass's first claim; requests no redraw.
    ///
    /// # The consumer contract
    /// 1. Claim from the `Move` arm on every move the local hit test passes, not just on entry.
    /// 2. A container claims *after* routing to its children: the first recorded claim wins.
    /// 3. Keep an internal hovered flag from that hit test; `request_redraw` only when it changes.
    /// 4. In `paint`, self-correct that flag from `PaintCtx::is_hovered`, which is authoritative.
    pub fn claim_hover(&mut self) {
        if self.hover_eligible {
            self.hover_claimed = true;
        }
    }

    /// Whether this widget or a descendant holds the hover link, as of before this dispatch;
    /// `PaintCtx::is_hovered` is the authoritative read for hover chrome.
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Ask the host to show `icon`; per pass, last writer wins, and a widget that stops asking
    /// falls back to [`CursorIcon::Default`]. Only a pointer `Move` (captured included) resolves
    /// it, so a drag re-asks on each captured `Move`, and a container overriding its child asks
    /// after routing (the reverse of `claim_hover`'s order). Requests no redraw.
    pub fn set_cursor(&mut self, icon: CursorIcon) {
        CURSOR_REQUEST.with(|slot| slot.set(Some(icon)));
    }

    /// Ask the shell to put `text` on the host clipboard — a widget's answer to `EditCommand::Copy`
    /// or `Cut`. Per pass, last writer wins; drained by
    /// [`RenderRoot::take_clipboard_write`](crate::app::RenderRoot::take_clipboard_write).
    pub fn write_clipboard(&mut self, text: String) {
        CLIPBOARD_WRITE.with(|slot| slot.set(Some(text)));
    }

    /// Ask the shell to read the host clipboard and dispatch it back as `EditCommand::Paste` on a
    /// later pass; idempotent per pass. The answer lands wherever focus is at delivery, so an async
    /// read binds it to [`RenderRoot::focus_epoch`](crate::app::RenderRoot::focus_epoch) and drops
    /// a mismatch (never `focus_ime_generation`, which edits also move).
    pub fn request_paste(&mut self) {
        PASTE_REQUEST.with(|slot| slot.set(true));
    }

    /// Queue `cmd` for whichever widget drains [`EventCtx::take_edit_commands`] later in this pass
    /// — the route from a floated toolbar back to its field. A command nobody takes is dropped.
    pub fn dispatch_edit_command(&mut self, cmd: EditCommand) {
        EDIT_COMMAND_QUEUE.with(|slot| {
            let mut queue = slot.take();
            queue.push(cmd);
            slot.set(queue);
        });
    }

    /// Drain this pass's queued edit commands in dispatch order; an overlay owner calls it right
    /// after forwarding into its floated pod.
    pub fn take_edit_commands(&mut self) -> Vec<EditCommand> {
        EDIT_COMMAND_QUEUE.with(|slot| slot.take())
    }

    /// Publish this widget's IME surface after an editing change; it bubbles up the focus chain to
    /// [`crate::app::RenderRoot::ime_state`], carried opaquely.
    pub fn publish_ime_state(&mut self, state: ImeState) {
        self.ime_state = Some(state);
    }

    /// Whether this widget requested focus during this (sub)dispatch (container-side).
    pub(crate) fn is_focus_requested(&self) -> bool {
        self.focus_requested
    }

    /// Whether this widget released focus during this (sub)dispatch (container-side).
    pub(crate) fn is_focus_released(&self) -> bool {
        self.focus_released
    }

    /// Take the IME surface published during this (sub)dispatch, leaving `None`.
    pub(crate) fn take_ime_state(&mut self) -> Option<ImeState> {
        self.ime_state.take()
    }

    /// Seed whether the receiving (root) widget holds focus.
    pub(crate) fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// Whether a widget claimed hover during this (sub)dispatch (container-side).
    pub(crate) fn is_hover_claimed(&self) -> bool {
        self.hover_claimed
    }

    /// Whether a claim would record; `ChildPod::event_child` narrows it before handing it down.
    pub(crate) fn is_hover_eligible(&self) -> bool {
        self.hover_eligible
    }

    /// Seed whether the receiving (root) widget holds the hover link.
    pub(crate) fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Seed whether this pass may record a claim (only an uncaptured `Move`).
    pub(crate) fn set_hover_eligible(&mut self, eligible: bool) {
        self.hover_eligible = eligible;
    }

    /// Seed the live hover epoch, threaded unchanged into every child.
    pub(crate) fn set_hover_epoch(&mut self, epoch: u64) {
        self.hover_epoch = epoch;
    }

    /// The live hover epoch: a pod whose stamp equals it still holds the link.
    pub(crate) fn hover_epoch(&self) -> u64 {
        self.hover_epoch
    }

    /// Seed the running root's identity, threaded unchanged into every child.
    pub(crate) fn set_hover_root(&mut self, root: u64) {
        self.hover_root = root;
    }

    /// The running root's identity, stamped beside the claim epoch so a dropped pod can qualify it.
    pub(crate) fn hover_root(&self) -> u64 {
        self.hover_root
    }

    /// The epoch a claim in this pass records: one past the live one, as the root advances at pass
    /// end. Wrapping is harmless; stamps are only compared for equality.
    pub(crate) fn hover_claim_epoch(&self) -> u64 {
        self.hover_epoch.wrapping_add(1)
    }

    /// The widget's origin in its **parent's** space, not window space (unlike `PaintCtx::origin`).
    /// Event positions are already local; compute window-anchored rects from `PaintCtx` in `paint`.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The receiving widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// A sub-context for a child at `origin`/`size` over the same state: request flags start
    /// clear; hover epoch, root identity and pointer id thread down. Folded back by `absorb_child`.
    pub(crate) fn child_ctx(
        &mut self,
        origin: Point,
        size: Size,
        focused: bool,
        hovered: bool,
        hover_eligible: bool,
    ) -> EventCtx<'_> {
        EventCtx {
            state: &mut *self.state,
            needs_redraw: false,
            needs_layout: false,
            layout_path_dirty: false,
            capture_requested: false,
            contacts_requested: false,
            capture_released: false,
            pointer_id: self.pointer_id,
            focus_requested: false,
            focus_released: false,
            has_focus: focused,
            hovered,
            hover_claimed: false,
            hover_eligible,
            hover_epoch: self.hover_epoch,
            hover_root: self.hover_root,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Drain this (child) context's request flags and IME surface into a [`ChildFold`].
    pub(crate) fn fold(&mut self) -> ChildFold {
        ChildFold {
            needs_redraw: self.needs_redraw,
            captured: self.capture_requested,
            contacts_requested: self.contacts_requested,
            capture_released: self.capture_released,
            hover_claimed: self.hover_claimed,
            focus_requested: self.focus_requested,
            focus_released: self.focus_released,
            ime_state: self.ime_state.take(),
        }
    }

    /// Fold a child dispatch's request flags and any published IME surface back into this context.
    pub(crate) fn absorb_child(&mut self, child: ChildFold) {
        self.needs_redraw |= child.needs_redraw;
        self.capture_requested |= child.captured;
        self.contacts_requested |= child.contacts_requested;
        self.capture_released |= child.capture_released;
        // A claim bubbles like a focus request, so the whole path carries the same stamp.
        self.hover_claimed |= child.hover_claimed;
        self.focus_requested |= child.focus_requested;
        self.focus_released |= child.focus_released;
        if child.ime_state.is_some() {
            self.ime_state = child.ime_state;
        }
    }
}
