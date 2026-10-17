//! Layer 2 input: pointer/scroll events and the [`EventCtx`] a widget mutates
//! while handling them.
//!
//! The pipeline mirrors Masonry's corrected pointer model: an [`InputEvent`]
//! enters the tree at the root ([`crate::app::RenderRoot::event`]) and is routed
//! down through container [`ChildPod`](crate::widget::ChildPod)s, each of which
//! translates the event into its child's local coordinate space before
//! forwarding. A widget reports what it did through [`EventResult`] and can, via
//! [`EventCtx`], mutate application state, request a redraw, or *capture* the
//! pointer so subsequent moves/releases route straight back to it.
//!
//! Capture here is **by recorded path**, not a global registry: on
//! [`PointerPhase::Down`] a widget calls [`EventCtx::capture_pointer`]; the
//! enclosing container reads the flag ([`EventCtx::is_pointer_captured`]) and
//! records which child was active so it can route later moves/releases directly.
//! Capture auto-releases on [`PointerPhase::Up`]/[`PointerPhase::Cancel`] (never
//! on window-leave) — the claimant's own, when more than one contact is down:
//! [`InputEvent::PointerContact`] states the multi-contact contract, and
//! [`EventCtx::pointer_id`]/[`EventCtx::capture_contacts`] are a widget's side of it.
//!
//! # Hover is a claim, not a phase
//!
//! There is no Enter/Leave phase, and [`PointerPhase`] deliberately gains none:
//! hover is an **opt-in claim** a widget makes from its ordinary uncaptured
//! [`PointerPhase::Move`] arm ([`EventCtx::claim_hover`]), recorded as a path
//! through the pod chain the same way focus is. The claim's identity is an
//! *epoch*: [`crate::app::RenderRoot`] advances one hover epoch per hover pass
//! (an uncaptured `Move`, or the `Down`/`Up`/`Cancel` that ends a hover
//! outright), a claim stamps that epoch onto every
//! [`ChildPod`](crate::widget::ChildPod) from the claimant up to the root, and a
//! link only counts as hovered while its stamp still matches the live epoch. So
//! the previous claimant needs no explicit clearing — the pointer moving anywhere
//! else advances the epoch and its stamp goes stale by construction, which is why
//! a container that never hears about the move cannot leave a stale path standing.
//!
//! Stranding needs a hover pass, and there is exactly one way for a link to lose
//! its owner without one: a rebuild that *removes* the claimant, which will never
//! see another `Move`. A dropped [`ChildPod`](crate::widget::ChildPod) holding the
//! live link therefore reports itself, and
//! [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild) ends the hover before
//! the frame does — the hover twin of the focus-orphan release.
//!
//! The recorded thing is a **path**, exactly like focus, and both hover reads
//! report membership of it: the claimant *and* every ancestor enclosing it read
//! hovered, the way CSS `:hover` applies to an element while the pointer is over
//! one of its descendants. Nothing off the path does — a sibling, or a widget
//! whose descendant did not claim, reads `false`.
//!
//! Only an **uncaptured** `Move` may claim: the root marks a captured pass
//! ineligible outright, and [`ChildPod::event_child`](crate::widget::ChildPod::event_child)
//! additionally refuses a claim from inside a pod that itself holds the capture
//! path, so a drag can never paint hover under the finger. At most one claim per
//! pass is recorded — the **first one recorded wins**, and every later claim in
//! that pass is ineligible — so at most one path is hovered and two *stacked*
//! widgets cannot each hold their own link. First-recorded is the topmost
//! (deepest) claimant only while every container claims **after** routing the
//! move to its children, which is what [`EventCtx::claim_hover`]'s contract
//! requires of one: an ancestor that claims *before* it forwards is recorded
//! first instead, and starves its whole subtree for the pass.
//!
//! # The cursor is a per-pass request, on its own channel
//!
//! [`EventCtx::set_cursor`] is hover's sibling and deliberately **not** derived
//! from it: the root's hover mirror is identity-free (it knows *that* something
//! is hovered, not which widget or what shape that widget wants), so the cursor
//! gets its own channel — one slot per pass, last writer wins, resolved by
//! [`crate::app::RenderRoot::event`] into [`crate::app::RenderRoot::cursor`] for
//! a desktop shell to apply. Absence resolves to [`CursorIcon::Default`], so a
//! widget that stops asking needs no clearing, and only a pointer
//! [`PointerPhase::Move`] re-resolves — a captured `Move` included, which is what
//! lets a drag keep its own cursor outside its bounds.

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
