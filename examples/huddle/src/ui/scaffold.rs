//! `scaffold`/`placeholder_body` — the shared titled-screen shell: the
//! screen's own [`app_bar`](frust_material::app_bar) at the top plus a centered body.
//! Promoted here from the former `screens` hub (now dissolved), since it is
//! consumed across three feature slices
//! (`messages::presentation::pages::thread`,
//! `search::presentation::pages::search`,
//! `profile::presentation::pages::profile`), not owned by a single screen —
//! mirrors [`crate::ui::fill_box`]'s promotion precedent.

use frust::{Align, Alignment, CrossAxisAlignment, View, column, text};
use frust_material::app_bar;

use crate::HuddleState;

/// A titled screen scaffold: the screen's own [`app_bar`](frust_material::app_bar) at
/// the top and a centered `body` filling the rest.
pub fn scaffold(title: &str, body: impl View<HuddleState>) -> impl View<HuddleState> {
    column()
        .child(app_bar::<HuddleState>(title))
        .flex(1, Align(Alignment::CENTER, body))
        .cross_axis(CrossAxisAlignment::Stretch)
}

/// A placeholder body: a big "…" plus a note.
pub fn placeholder_body(note: &str) -> impl View<HuddleState> + use<> {
    column()
        .child(text("\u{2026}").size(48.0))
        .child(text(note).size(14.0))
}
