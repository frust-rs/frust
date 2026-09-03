//! Home: the gallery's hero page — title, upstream attribution, and a
//! category-overview card per section, each a live link into that section's
//! first page.

use frust::{Column, CrossAxisAlignment, Row, SizedBox, View, any, text};
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
    any(Column(vec![
        any(text(title.to_string()).size(16.0)),
        any(SizedBox(None, Some(4.0))),
        any(caption(summary.to_string())),
        any(SizedBox(None, Some(8.0))),
        any(button("Open", move |s: &mut AppState| s.page = target).tone(ButtonTone::Outline)),
    ]))
}

pub fn page(_state: &mut State) -> impl View<AppState> + use<> {
    Column(vec![
        any(heading("beUI Gallery")),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "A desktop gallery for frust_beui — beUI v2 ported from \
             github.com/starc007/ui-components (MIT, \u{a9} 2026 Saurabh Chauhan), \
             rev 10c283e433a8f4f0ac0736684d4426ab612b9f55, retrieved 2026-09-01.",
        )),
        any(SizedBox(None, Some(24.0))),
        any(
            Row(vec![
                category_card(
                    "Motion",
                    "9 pages covering the 41 Phase-2 motion components: text, buttons, controls, selection, overlays, navigation, surfaces, data, and a WGSL shader page.",
                    Page::MotionText,
                ),
                any(SizedBox(Some(16.0), None)),
                category_card(
                    "Agents",
                    "Chat-app primitives, panels, and a full chat demo.",
                    Page::AgentsPrimitives,
                ),
                any(SizedBox(Some(16.0), None)),
                category_card(
                    "Blocks",
                    "Command palette, morph modal, forms, and a showcase.",
                    Page::BlocksCommand,
                ),
                any(SizedBox(Some(16.0), None)),
                category_card(
                    "Theming",
                    "The beUI palette across both brightnesses, a live scheme toggle, and typography specimens.",
                    Page::Theming,
                ),
            ])
            .cross_axis(CrossAxisAlignment::Start),
        ),
    ])
}
