//! Layout: the space-dividing components — `carousel` (default full-width
//! pager and a one-third-basis variant), `resizable` (a controlled horizontal
//! group with grips and an uncontrolled vertical one), and a second
//! `sidebar_provider` in its non-default `floating` variant on the right-hand
//! side.
//!
//! All three need a **finite** height to divide, so each demo sits in its own
//! `SizedBox` — this page itself scrolls, and a scroll view hands its child an
//! unbounded height.

use frust::{AnyView, Column, EdgeInsets, Padding, SizedBox, View, any, icon, icons, text};
use frust_shadcn::{
    ButtonVariant, ResizableDirection, SidebarCollapsible, SidebarSide, SidebarVariant, button,
    card, card_content, card_header, card_title, carousel, resizable_handle, resizable_panel,
    resizable_panel_group, separator, sidebar, sidebar_content, sidebar_group, sidebar_group_label,
    sidebar_header, sidebar_inset, sidebar_menu, sidebar_menu_button, sidebar_menu_item,
    sidebar_provider, sidebar_trigger,
};

use crate::AppState;

pub struct State {
    pub carousel_index: usize,
    /// The controlled split of the horizontal resizable group.
    pub split: Vec<f64>,
    pub mini_open: bool,
    pub mini_item: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            carousel_index: 0,
            split: vec![0.3, 0.45, 0.25],
            mini_open: true,
            mini_item: 0,
        }
    }
}

const MINI_ITEMS: [&str; 3] = ["Inbox", "Drafts", "Archive"];

fn gap() -> AnyView<AppState> {
    any(SizedBox(None, Some(16.0)))
}

/// One carousel slide: a card with a large ordinal.
fn slide(index: usize) -> AnyView<AppState> {
    any(card(vec![
        card_header(vec![card_title(format!("Slide {}", index + 1))]),
        card_content(text("Drag the slide sideways, or use the outline arrows.").size(13.0)),
    ]))
}

/// One resizable panel's filling: a padded label on the panel's own surface.
fn panel_body(label: &str, hint: &str) -> impl View<AppState> + use<> {
    Padding(
        EdgeInsets::all(12.0),
        Column(vec![
            any(text(label.to_string()).size(14.0)),
            any(SizedBox(None, Some(4.0))),
            any(text(hint.to_string()).size(12.0)),
        ]),
    )
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let carousel_index = state.carousel_index;
    let split = state.split.clone();
    let mini_open = state.mini_open;
    let mini_item = state.mini_item;

    let slides: Vec<AnyView<AppState>> = (0..5).map(slide).collect();
    let thirds: Vec<AnyView<AppState>> = (0..6)
        .map(|i| {
            any(card(vec![card_content(
                text(format!("Item {}", i + 1)).size(13.0),
            )]))
        })
        .collect();

    Column(vec![
        any(crate::nav::heading("Layout")),
        any(SizedBox(None, Some(16.0))),
        // --- Carousel: controlled, full-width slides ---
        any(crate::nav::caption(format!(
            "Carousel \u{2014} controlled: slide {} of 5. Drag a slide and let go to \
             snap; a flick carries one further. The outline arrows disable at the \
             ends, and Left/Right page it once it holds focus.",
            carousel_index + 1
        ))),
        any(SizedBox(None, Some(8.0))),
        any(SizedBox::<AppState>(None, Some(170.0)).child(
            carousel(slides).selected(carousel_index).on_select(
                |s: &mut AppState, index: usize| {
                    s.layout.carousel_index = index;
                },
            ),
        )),
        gap(),
        // --- Carousel: the non-default basis, uncontrolled ---
        any(crate::nav::caption(
            "Carousel \u{2014} `item_fraction(1/3)`, uncontrolled (it owns its own \
             index, like embla's default).",
        )),
        any(SizedBox(None, Some(8.0))),
        any(SizedBox::<AppState>(None, Some(120.0))
            .child(carousel(thirds).item_fraction(1.0 / 3.0))),
        gap(),
        any(separator()),
        gap(),
        // --- Resizable: controlled horizontal group with grips ---
        any(crate::nav::caption(format!(
            "Resizable \u{2014} controlled: {}. Drag a seam (the cursor turns into a \
             column resize over it); Tab to a seam and nudge it with the arrow keys. \
             Panels clamp at their own min/max.",
            split
                .iter()
                .map(|f| format!("{:.0}%", f * 100.0))
                .collect::<Vec<_>>()
                .join(" / ")
        ))),
        any(SizedBox(None, Some(8.0))),
        any(SizedBox::<AppState>(None, Some(200.0)).child(
            resizable_panel_group(vec![
                resizable_panel(panel_body("Sidebar", "min 15%")).min_size(0.15),
                resizable_panel(panel_body("Content", "min 25%")).min_size(0.25),
                resizable_panel(panel_body("Inspector", "min 15%, max 40%"))
                    .min_size(0.15)
                    .max_size(0.4),
            ])
            .handle(resizable_handle().with_grip(true))
            .sizes(split)
            .on_layout(|s: &mut AppState, sizes: Vec<f64>| {
                s.layout.split = sizes;
            }),
        )),
        gap(),
        // --- Resizable: the non-default axis, uncontrolled, bare seams ---
        any(crate::nav::caption(
            "Resizable \u{2014} `direction(Vertical)`, uncontrolled, bare 1px seams \
             (no grip): the cursor turns into a row resize instead.",
        )),
        any(SizedBox(None, Some(8.0))),
        any(SizedBox::<AppState>(None, Some(200.0)).child(
            resizable_panel_group(vec![
                resizable_panel(panel_body("Top", "default 60%")).default_size(0.6),
                resizable_panel(panel_body("Bottom", "default 40%")).default_size(0.4),
            ])
            .direction(ResizableDirection::Vertical),
        )),
        gap(),
        any(separator()),
        gap(),
        // --- A second sidebar, in its non-default chrome ---
        any(crate::nav::caption(
            "Sidebar \u{2014} the same component as the app's own nav, in its \
             `floating` variant docked to the right, with its own open state and \
             its own trigger. Ctrl/Cmd+B reaches whichever provider holds focus.",
        )),
        any(SizedBox(None, Some(8.0))),
        any(
            SizedBox::<AppState>(None, Some(300.0)).child(sidebar_provider(
                sidebar(sidebar_content(vec![sidebar_group(vec![
                    any(sidebar_group_label("Mail")),
                    sidebar_menu(
                        MINI_ITEMS
                            .iter()
                            .enumerate()
                            .map(|(index, label)| {
                                any(sidebar_menu_item(vec![any(sidebar_menu_button(
                                    *label,
                                    move |s: &mut AppState| {
                                        s.layout.mini_item = index;
                                    },
                                )
                                .icon(icon(icons::FORUM).size(16.0))
                                .active(index == mini_item))]))
                            })
                            .collect(),
                    ),
                ])]))
                .header(sidebar_header(vec![any(text("Floating").size(14.0))]))
                .side(SidebarSide::Right)
                .variant(SidebarVariant::Floating)
                .collapsible(SidebarCollapsible::Icon)
                .rail(true),
                sidebar_inset(vec![any(Padding(
                    EdgeInsets::all(16.0),
                    Column(vec![
                        any(sidebar_trigger(
                            mini_open,
                            |s: &mut AppState, next: bool| {
                                s.layout.mini_open = next;
                            },
                        )),
                        any(SizedBox(None, Some(12.0))),
                        any(text(format!("Selected: {}", MINI_ITEMS[mini_item])).size(14.0)),
                        any(SizedBox(None, Some(12.0))),
                        any(button("Toggle from app code", |s: &mut AppState| {
                            s.layout.mini_open = !s.layout.mini_open;
                        })
                        .variant(ButtonVariant::Outline)),
                    ]),
                ))]),
                mini_open,
                |s: &mut AppState, next: bool| {
                    s.layout.mini_open = next;
                },
            )),
        ),
    ])
}
