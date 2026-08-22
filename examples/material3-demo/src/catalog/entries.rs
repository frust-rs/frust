//! The catalog itself: 39 entries, five sections, in the reference's own
//! order.
//!
//! Each entry's `build` names its page module's `page` fn directly, so adding
//! a component means adding one row here and one file under
//! [`crate::pages::playground`].

use frust_material::icons;

use crate::catalog::{DemoEntry, DemoSection};
use crate::pages::playground::{do_, find, nav, pick, view};

/// Do (Actions).
pub const DO_ENTRIES: [DemoEntry; 7] = [
    DemoEntry {
        id: "buttons",
        title: "Buttons",
        subtitle: "Filled, tonal, elevated, outlined, text",
        icon: icons::SMART_BUTTON,
        section: DemoSection::Do,
        build: do_::buttons::page,
    },
    DemoEntry {
        id: "icon_buttons",
        title: "Icon buttons",
        subtitle: "Variants, sizes, toggle, badge",
        icon: icons::FAVORITE,
        section: DemoSection::Do,
        build: do_::icon_buttons::page,
    },
    DemoEntry {
        id: "fabs",
        title: "FABs",
        subtitle: "FAB and extended FAB",
        icon: icons::ADD,
        section: DemoSection::Do,
        build: do_::fabs::page,
    },
    DemoEntry {
        id: "fab_menu",
        title: "FAB menu",
        subtitle: "Speed-dial style menu",
        icon: icons::APPS,
        section: DemoSection::Do,
        build: do_::fab_menu::page,
    },
    DemoEntry {
        id: "button_group",
        title: "Button group",
        subtitle: "Connected groups and toggle buttons",
        icon: icons::VIEW_WEEK,
        section: DemoSection::Do,
        build: do_::button_group::page,
    },
    DemoEntry {
        id: "segmented_button",
        title: "Segmented button",
        subtitle: "Single and multi select segments",
        icon: icons::VIEW_COLUMN,
        section: DemoSection::Do,
        build: do_::segmented_button::page,
    },
    DemoEntry {
        id: "split_button",
        title: "Split button",
        subtitle: "Primary action plus menu",
        icon: icons::ARROW_DROP_DOWN,
        section: DemoSection::Do,
        build: do_::split_button::page,
    },
];

/// Pick (Selection).
pub const PICK_ENTRIES: [DemoEntry; 8] = [
    DemoEntry {
        id: "checkbox",
        title: "Checkbox",
        subtitle: "Binary, tristate, error",
        icon: icons::CHECK_BOX,
        section: DemoSection::Pick,
        build: pick::checkbox::page,
    },
    DemoEntry {
        id: "radio",
        title: "Radio",
        subtitle: "Exclusive selection",
        icon: icons::RADIO_BUTTON_CHECKED,
        section: DemoSection::Pick,
        build: pick::radio::page,
    },
    DemoEntry {
        id: "switch",
        title: "Switch",
        subtitle: "On/off with optional icons",
        icon: icons::TOGGLE_ON,
        section: DemoSection::Pick,
        build: pick::switch::page,
    },
    DemoEntry {
        id: "chips",
        title: "Chips",
        subtitle: "Assist, filter, input, suggestion",
        icon: icons::LABEL,
        section: DemoSection::Pick,
        build: pick::chips::page,
    },
    DemoEntry {
        id: "dropdown_menu",
        title: "Dropdown menu",
        subtitle: "Single, multi, search, async",
        icon: icons::ARROW_DROP_DOWN_CIRCLE,
        section: DemoSection::Pick,
        build: pick::dropdown_menu::page,
    },
    DemoEntry {
        id: "sliders",
        title: "Sliders",
        subtitle: "Continuous, discrete, range, wavy",
        icon: icons::LINEAR_SCALE,
        section: DemoSection::Pick,
        build: pick::sliders::page,
    },
    DemoEntry {
        id: "date_pickers",
        title: "Date pickers",
        subtitle: "Calendar and dialogs",
        icon: icons::CALENDAR_TODAY,
        section: DemoSection::Pick,
        build: pick::date_pickers::page,
    },
    DemoEntry {
        id: "time_pickers",
        title: "Time pickers",
        subtitle: "Dial and dialog",
        icon: icons::SCHEDULE,
        section: DemoSection::Pick,
        build: pick::time_pickers::page,
    },
];

/// View (Containment).
pub const VIEW_ENTRIES: [DemoEntry; 9] = [
    DemoEntry {
        id: "cards",
        title: "Cards",
        subtitle: "Elevated, filled, outlined",
        icon: icons::CROP_SQUARE,
        section: DemoSection::View,
        build: view::cards::page,
    },
    DemoEntry {
        id: "carousel",
        title: "Carousel",
        subtitle: "Hero and contained layouts",
        icon: icons::VIEW_CAROUSEL,
        section: DemoSection::View,
        build: view::carousel::page,
    },
    DemoEntry {
        id: "lists",
        title: "Lists",
        subtitle: "Item, card, dismissible, expandable",
        icon: icons::LIST,
        section: DemoSection::View,
        build: view::lists::page,
    },
    DemoEntry {
        id: "selection",
        title: "Selection",
        subtitle: "Multi-select host and app bar",
        icon: icons::SELECT_ALL,
        section: DemoSection::View,
        build: view::selection::page,
    },
    DemoEntry {
        id: "dividers",
        title: "Dividers",
        subtitle: "Horizontal and vertical",
        icon: icons::HORIZONTAL_RULE,
        section: DemoSection::View,
        build: view::dividers::page,
    },
    DemoEntry {
        id: "shapes",
        title: "Shapes",
        subtitle: "Expressive clip catalog",
        icon: icons::CATEGORY,
        section: DemoSection::View,
        build: view::shapes::page,
    },
    DemoEntry {
        id: "dialogs",
        title: "Dialogs",
        subtitle: "Basic, selection, full-screen",
        icon: icons::CHAT_BUBBLE,
        section: DemoSection::View,
        build: view::dialogs::page,
    },
    DemoEntry {
        id: "bottom_sheet",
        title: "Bottom sheet",
        subtitle: "Modal sheet with drag handle",
        icon: icons::VERTICAL_ALIGN_BOTTOM,
        section: DemoSection::View,
        build: view::bottom_sheet::page,
    },
    DemoEntry {
        id: "side_sheet",
        title: "Side sheet",
        subtitle: "Side panel with actions",
        icon: icons::VERTICAL_SPLIT,
        section: DemoSection::View,
        build: view::side_sheet::page,
    },
];

/// Nav (Navigation).
pub const NAV_ENTRIES: [DemoEntry; 7] = [
    DemoEntry {
        id: "app_bars",
        title: "App bars",
        subtitle: "Top, search, sliver, bottom",
        icon: icons::WEB_ASSET,
        section: DemoSection::Nav,
        build: nav::app_bars::page,
    },
    DemoEntry {
        id: "tabs",
        title: "Tabs",
        subtitle: "Primary and secondary",
        icon: icons::TAB,
        section: DemoSection::Nav,
        build: nav::tabs::page,
    },
    DemoEntry {
        id: "navigation_bar",
        title: "Navigation bar",
        subtitle: "Bottom destinations",
        icon: icons::SPACE_DASHBOARD,
        section: DemoSection::Nav,
        build: nav::navigation_bar::page,
    },
    DemoEntry {
        id: "navigation_rail",
        title: "Navigation rail",
        subtitle: "Side rail with sections",
        icon: icons::VIEW_SIDEBAR,
        section: DemoSection::Nav,
        build: nav::navigation_rail::page,
    },
    DemoEntry {
        id: "navigation_drawer",
        title: "Navigation drawer",
        subtitle: "Modal drawer destinations",
        icon: icons::MENU,
        section: DemoSection::Nav,
        build: nav::navigation_drawer::page,
    },
    DemoEntry {
        id: "toolbar",
        title: "Toolbar",
        subtitle: "Floating and docked toolbars",
        icon: icons::BUILD,
        section: DemoSection::Nav,
        build: nav::toolbar::page,
    },
    DemoEntry {
        id: "menu",
        title: "Menu",
        subtitle: "Anchored expressive menus",
        icon: icons::MORE_VERT,
        section: DemoSection::Nav,
        build: nav::menu::page,
    },
];

/// Find (Feedback / input).
pub const FIND_ENTRIES: [DemoEntry; 8] = [
    DemoEntry {
        id: "badges",
        title: "Badges",
        subtitle: "Dot and count badges",
        icon: icons::NOTIFICATIONS,
        section: DemoSection::Find,
        build: find::badges::page,
    },
    DemoEntry {
        id: "progress",
        title: "Progress indicators",
        subtitle: "Linear, circular, wavy",
        icon: icons::HOURGLASS_EMPTY,
        section: DemoSection::Find,
        build: find::progress::page,
    },
    DemoEntry {
        id: "loading_indicator",
        title: "Loading indicator",
        subtitle: "Expressive loading",
        icon: icons::AUTORENEW,
        section: DemoSection::Find,
        build: find::loading_indicator::page,
    },
    DemoEntry {
        id: "refresh_indicator",
        title: "Refresh indicator",
        subtitle: "Pull to refresh",
        icon: icons::REFRESH,
        section: DemoSection::Find,
        build: find::refresh_indicator::page,
    },
    DemoEntry {
        id: "tooltips",
        title: "Tooltips",
        subtitle: "Plain and rich tooltips",
        icon: icons::INFO,
        section: DemoSection::Find,
        build: find::tooltips::page,
    },
    DemoEntry {
        id: "snackbar",
        title: "Snackbar",
        subtitle: "Transient messages",
        icon: icons::MESSAGE,
        section: DemoSection::Find,
        build: find::snackbar::page,
    },
    DemoEntry {
        id: "text_fields",
        title: "Text fields",
        subtitle: "Filled and outlined",
        icon: icons::TEXT_FIELDS,
        section: DemoSection::Find,
        build: find::text_fields::page,
    },
    DemoEntry {
        id: "search",
        title: "Search",
        subtitle: "Search bar and anchor",
        icon: icons::SEARCH,
        section: DemoSection::Find,
        build: find::search::page,
    },
];

/// Every entry filed under `section`, in list order.
pub fn for_section(section: DemoSection) -> &'static [DemoEntry] {
    match section {
        DemoSection::Do => &DO_ENTRIES,
        DemoSection::Pick => &PICK_ENTRIES,
        DemoSection::View => &VIEW_ENTRIES,
        DemoSection::Nav => &NAV_ENTRIES,
        DemoSection::Find => &FIND_ENTRIES,
    }
}

/// Every entry in the catalog, section by section.
pub fn all() -> impl Iterator<Item = DemoEntry> {
    DemoSection::ALL
        .into_iter()
        .flat_map(|section| for_section(section).iter().copied())
}

/// The entry with `id`, or `None` — the `/playground/:id` lookup.
pub fn find_by_id(id: &str) -> Option<DemoEntry> {
    all().find(|entry| entry.id == id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{all, find_by_id, for_section};
    use crate::catalog::DemoSection;

    /// The reference catalog's own size — 7/8/9/7/8 across the five sections.
    const EXPECTED_ENTRY_COUNT: usize = 39;

    #[test]
    fn the_catalog_holds_every_reference_entry() {
        assert_eq!(all().count(), EXPECTED_ENTRY_COUNT);
        let per_section: Vec<usize> = DemoSection::ALL
            .into_iter()
            .map(|section| for_section(section).len())
            .collect();
        assert_eq!(per_section, vec![7, 8, 9, 7, 8]);
    }

    #[test]
    fn every_id_is_unique_and_findable() {
        let mut seen = HashSet::new();
        for entry in all() {
            assert!(seen.insert(entry.id), "duplicate catalog id: {}", entry.id);
            assert!(find_by_id(entry.id).is_some());
        }
        assert!(find_by_id("no-such-entry").is_none());
    }

    #[test]
    fn every_entry_is_filed_under_the_section_that_lists_it() {
        for section in DemoSection::ALL {
            for entry in for_section(section) {
                assert_eq!(entry.section, section, "{} is misfiled", entry.id);
            }
        }
    }

    #[test]
    fn every_entry_carries_a_title_and_a_subtitle() {
        for entry in all() {
            assert!(!entry.title.is_empty(), "{} has no title", entry.id);
            assert!(!entry.subtitle.is_empty(), "{} has no subtitle", entry.id);
        }
    }
}
