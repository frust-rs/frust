//! Every Huddle screen module, declared here in the skeleton (task 10).
//!
//! This `mod.rs` is a **hub file** — Phase C screen tasks edit ONLY their own
//! `screens/<name>.rs` (and their `features/<name>/**`), never this file, nor
//! `lib.rs`/`shell.rs`/`routes.rs`/`mock/`/`ui/` (see `src/README-phase-c.md`).
//! Each screen below is a **placeholder** carrying a doc comment naming the
//! Phase C task that fills it; the only non-placeholder is
//! [`settings_appearance`], which already hosts the live theme-swap mechanism.
//!
//! Every screen renders and navigates TODAY via the shared [`scaffold`] helper:
//! its own [`app_bar`](frust::app_bar) plus a centered body.

pub mod activity;
pub mod profile;
pub mod search;
pub mod settings;
pub mod settings_about;
pub mod settings_appearance;
pub mod settings_notifications;
pub mod you;

use frust::{
    Align, Alignment, AnyView, Axis, Column, CrossAxisAlignment, FlexView, any, app_bar, flexible,
    inflexible, text,
};

use crate::HuddleState;

/// A titled screen scaffold: the screen's own [`app_bar`](frust::app_bar) at
/// the top and a centered `body` filling the rest. The shared shape every
/// placeholder screen (and many Phase C screens) is built from.
pub fn scaffold(title: &str, body: AnyView<HuddleState>) -> AnyView<HuddleState> {
    any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(any(app_bar::<HuddleState>(title))),
            flexible(1, any(Align(Alignment::CENTER, body))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}

/// A placeholder body: a big "…" plus a note describing what Phase C will build
/// here. Used by every screen that isn't wired up yet.
pub fn placeholder_body(note: &str) -> AnyView<HuddleState> {
    any(Column(vec![
        any(text("\u{2026}").size(48.0)),
        any(text(note).size(14.0)),
    ]))
}
