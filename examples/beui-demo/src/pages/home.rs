//! Home: the gallery's hero page — title, upstream attribution, and a
//! category-overview card per section, each a live link into that section's
//! first page.

use frust::{CrossAxisAlignment, SizedBox, View, any, column, row, text};
use frust_beui::components::button::{ButtonTone, button};

use crate::AppState;
use crate::nav::{Page, caption, heading};

/// Home carries no interactive state of its own — every action on this page
/// writes straight into [`crate::AppState::page`].
#[derive(Default)]
pub struct State;

/// One category card: a title, a one-line summary, and an "Open" button that
/// jumps straight to the section's first page.
fn category_card(title: &str, summary: &str, target: Page) -> frust::AnyView<AppState> {
    any(column()
        .child(text(title.to_string()).size(16.0))
        .child(SizedBox(None, Some(4.0)))
        .child(caption(summary.to_string()))
        .child(SizedBox(None, Some(8.0)))
        .child(button("Open", move |s: &mut AppState| s.page = target).tone(ButtonTone::Outline)))
}

pub fn page(_state: &mut State) -> impl View<AppState> + use<> {
    column()
        .child(heading("beUI Gallery"))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "A desktop gallery for frust_beui — beUI v2 ported from \
             github.com/starc007/ui-components (MIT, \u{a9} 2026 Saurabh Chauhan), \
             rev 10c283e433a8f4f0ac0736684d4426ab612b9f55, retrieved 2026-09-01.",
        ))
        .child(SizedBox(None, Some(24.0)))
        .child(row()
                .child(category_card(
                    "Motion",
                    "9 pages covering the 41 motion components: text, buttons, controls, selection, overlays, navigation, surfaces, data, and a WGSL shader page.",
                    Page::MotionText,
                ))
                .child(SizedBox(Some(16.0), None))
                .child(category_card(
                    "Agents",
                    "Chat-app primitives, panels, and a full chat demo.",
                    Page::AgentsPrimitives,
                ))
                .child(SizedBox(Some(16.0), None))
                .child(category_card(
                    "Blocks",
                    "Command palette, morph modal, forms, and a showcase.",
                    Page::BlocksCommand,
                ))
                .child(SizedBox(Some(16.0), None))
                .child(category_card(
                    "Theming",
                    "The beUI palette across both brightnesses, a live scheme toggle, and typography specimens.",
                    Page::Theming,
                ))
            .cross_axis(CrossAxisAlignment::Start))
}
