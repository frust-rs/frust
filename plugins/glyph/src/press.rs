//! The catalog's pointer-button admission test.
//!
//! Every interactive component in this crate asks the same question before it
//! enters a press/activation state — "may this button start a press?"
//! ([`presses`]) — so the catalog has exactly one answer to it.

use frust::authoring::{PointerButton, PointerEvent};

/// Whether `p` carries a button that may begin a press.
///
/// A press/activation machine — a `pressed` visual or state layer, a pointer
/// capture, an up-inside callback, a drag, a scrim-dismiss arming — starts on
/// the **primary** button alone: the left mouse button, or any touch/pen
/// contact (which every shell reports as [`PointerButton::Primary`] too). A
/// secondary press is a context gesture, and no component in this catalog does
/// anything with it, so it is left for a context-menu consumer instead of
/// activating the control under the cursor.
///
/// A modal barrier (a dialog/sheet/menu scrim) still **swallows** a secondary
/// press — it blocks the page behind it whatever button pressed — it just never
/// arms a dismiss from one.
///
/// Move/hover arms deliberately do **not** consult this: hover chrome and
/// cursor shapes are position-driven, and a move carries no meaningful button.
/// Neither do the focus-session arms (`request_focus`, an IME publish): the
/// root reads any `Down` that bubbles no claim as a blur, whatever button
/// carried it, so a secondary press inside an open overlay must still re-claim.
///
/// `frust_shadcn`'s `hit::presses` and `frust_widgets`' `authoring::presses` are
/// the same predicate for their own trees; each catalog keeps its own copy
/// rather than depending on a sibling design-system crate.
pub(crate) fn presses(p: &PointerEvent) -> bool {
    p.button == PointerButton::Primary
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{Point, PointerPhase};

    fn at(button: PointerButton) -> PointerEvent {
        PointerEvent {
            phase: PointerPhase::Down,
            position: Point::ORIGIN,
            button,
        }
    }

    #[test]
    fn only_the_primary_button_presses() {
        assert!(presses(&at(PointerButton::Primary)));
        assert!(!presses(&at(PointerButton::Secondary)));
        assert!(!presses(&at(PointerButton::Middle)));
    }
}
