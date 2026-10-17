//! Layer 2 input: the [`InputEvent`] vocabulary and the [`EventCtx`] a widget mutates handling it.
//!
//! An event enters at [`crate::app::RenderRoot::event`] and is routed down through each
//! [`ChildPod`](crate::widget::ChildPod), translated into the child's local space. A widget reports
//! an [`EventResult`] and, through [`EventCtx`], mutates state, requests a redraw, or captures the
//! pointer — by recorded path, released by the claimant's `Up`/`Cancel`, never by window-leave.
//!
//! # Hover is a claim, not a phase
//!
//! There is no Enter/Leave: a widget calls [`EventCtx::claim_hover`] from an uncaptured
//! [`PointerPhase::Move`]. The root advances one hover epoch per hover pass and a claim stamps it
//! on every pod up to the root, so an unclaimed link lapses by construction. The claimant and every
//! ancestor read hovered (CSS `:hover`); the first claim recorded in a pass wins, so a container
//! claims after routing. A rebuild that drops the live claimant ends the hover.
//!
//! # The cursor is a per-pass request
//!
//! [`EventCtx::set_cursor`] fills one slot per pass, last writer wins; only a pointer `Move`
//! (captured included) re-resolves [`crate::app::RenderRoot::cursor`], and an empty slot resolves
//! to [`CursorIcon::Default`].
//!
//! See `docs/CORE_ARCHITECTURE.md` § Data Flow.

mod contact;
mod ctx;
mod flags;
mod ime;
mod input;
mod key;
mod pass;
mod pointer;
#[cfg(test)]
mod tests;

pub(crate) use contact::{
    ContactFrame, ContactPass, current_pointer_id, in_secondary_contact_pass,
    note_secondary_delivered, run_secondary_walk, secondary_walk_carrier, secondary_walk_event,
    without_secondary_walk,
};
pub use ctx::{EventCtx, EventOutcome, EventResult};
pub use flags::{
    has_pending_result_flush, mark_focus_orphaned, mark_pending_result_flush, take_focus_orphaned,
    take_pending_result_flush,
};
pub(crate) use flags::{
    live_hover_link_is, mark_hover_orphaned, set_live_hover_link, take_hover_orphaned,
};
pub use ime::{ImeContentType, ImeEvent, ImeState};
pub use input::{FileDropEvent, FileDropPhase, InputEvent, OverlayEvent, OverlayEventKind};
pub use key::{EditCommand, EditingState, Key, KeyEvent, Modifiers, NamedKey};
pub(crate) use pass::{PassBracket, RequestPass};
#[cfg(test)]
pub(crate) use pass::{clear_cursor_request, take_cursor_request};
pub use pointer::{
    CursorIcon, PointerButton, PointerEvent, PointerId, PointerPhase, PointerSource, ScaleEvent,
    ScalePhase, ScrollDelta,
};
