//! **TabBar** — a bare native `UITabBar` (`native_tab_bar`) docked at the
//! bottom of this page, beside Glyph's drawn `tabs` strip on the same
//! selection.
//!
//! Three items cover the item vocabulary: a byte icon (the demo PNG, tinted
//! as a template), an SF Symbol with a distinct selected-state symbol, and a
//! badged item. The bar is controlled: `.on_select` reports the requested tab
//! and the page feeds the confirmed id back; `.on_reselect` counts taps on
//! the tab already showing.
//!
//! # A page-local index, not the app router
//!
//! Selection drives a page-local signal whose value picks the "tab page"
//! content above the bar — deliberately NOT the shell's section navigation.
//! This page exists to show the control, and routing the app from it would
//! navigate away from the control under test (the shell's own navigation is
//! frust-drawn precisely because this bar is iOS-only). A real app would feed
//! the same confirmed id to its router (`RouteNavigator::go`), as the plugin
//! README shows.
//!
//! # Placement
//!
//! The bar is the page column's last child, but this page sits inside the
//! shell's scroll view above the section navigation — it is not docked to the
//! window's bottom edge — so it is built with `.safe_area(false)`: the bare
//! 49pt, without growing by the home-indicator inset.
//!
//! macOS has no bottom-tab idiom and Android's `BottomNavigationView` needs
//! Material, so both (and any desktop preview) render the plugin's refusal
//! banner in place of the bar. One native slot at rest on iOS/iPadOS, none
//! elsewhere.

use frust::{AnyView, Get, Set, any, inflexible};
use frust_glyph::tabs;
use frust_native_widgets::{TabIcon, TabId, TabItem, native_tab_bar};

use super::common::{
    S, block, bump, caption, demo_image_bytes, gap, label, local_sig, page_column, page_header,
    readout,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 4;

/// The tabs: stable id, title, and what the page shows while selected.
const TABS: [(&str, &str, &str); 3] = [
    (
        "home",
        "Home",
        "Home \u{2014} a byte icon: the demo PNG, shown as a template image the bar tints.",
    ),
    (
        "search",
        "Search",
        "Search \u{2014} an SF Symbol (magnifyingglass), with a filled selected-state symbol.",
    ),
    (
        "inbox",
        "Inbox",
        "Inbox \u{2014} an SF Symbol (tray) carrying a \u{201c}3\u{201d} badge.",
    ),
];

local_sig!(selected_sig, usize, 0);
local_sig!(select_events_sig, u32, 0);
local_sig!(reselect_sig, u32, 0);

/// The index of the tab carrying `id`, if any.
fn index_of(id: &str) -> Option<usize> {
    TABS.iter().position(|(tab, _, _)| *tab == id)
}

/// The native items: byte icon, symbol with a selected symbol, badged.
fn items() -> Vec<TabItem> {
    vec![
        TabItem::new(TABS[0].0, TABS[0].1, TabIcon::Bytes(demo_image_bytes())),
        TabItem::new(
            TABS[1].0,
            TABS[1].1,
            TabIcon::AppleSymbol("magnifyingglass".to_string()),
        )
        .selected_icon(TabIcon::AppleSymbol(
            "magnifyingglass.circle.fill".to_string(),
        )),
        TabItem::new(
            TABS[2].0,
            TABS[2].1,
            TabIcon::AppleSymbol("tray".to_string()),
        )
        .badge("3"),
    ]
}

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
    let selected = selected_sig().get().min(TABS.len() - 1);
    let select_events = select_events_sig().get();
    let reselects = reselect_sig().get();

    let tab_page = block(vec![
        inflexible(label(format!("Tab page: {}", TABS[selected].1))),
        gap(4.0),
        inflexible(caption(TABS[selected].2)),
        gap(6.0),
        inflexible(readout(format!(
            "Selected: {} \u{2014} select events {select_events}, reselects {reselects}",
            TABS[selected].0
        ))),
        gap(4.0),
        inflexible(caption(
            "The selection is a page-local index, not the app router: routing from here would \
             navigate away from the control under test. Tap the showing tab again to count a \
             reselect (the conventional scroll-to-top).",
        )),
    ]);

    let drawn = block(vec![
        inflexible(label("Drawn peer: Glyph tabs on the same selection")),
        gap(6.0),
        inflexible(any(tabs(
            TABS.iter().map(|(_, title, _)| title.to_string()).collect(),
            selected,
            |_: &mut S, index: usize| {
                if index < TABS.len() {
                    selected_sig().set(index);
                }
            },
        ))),
    ]);

    let native_intro = block(vec![
        inflexible(label("Native tab bar (docked at the page bottom)")),
        gap(4.0),
        inflexible(caption(
            "Android: none (refusal banner) \u{b7} iOS: UITabBar (bare, no controller) \u{b7} \
             macOS: none (refusal banner). Built with .safe_area(false): this page is not \
             docked to the window's bottom edge.",
        )),
    ]);

    let bar = native_tab_bar(items(), TabId::new(TABS[selected].0))
        .safe_area(false)
        .on_select(|id| {
            bump(select_events_sig());
            if let Some(index) = index_of(id.as_str()) {
                selected_sig().set(index);
            }
        })
        .on_reselect(|_| bump(reselect_sig()));

    page_column(vec![
        page_header(SECTION),
        tab_page,
        drawn,
        native_intro,
        inflexible(any(bar)),
    ])
}
