//! The persistent Huddle shell: the 4-tab bottom navigation
//! (Home / Search / Activity / You) and the shared [`Tab`] enum every tab
//! routes on.
//!
//! This is a **hub file finalized in the skeleton (task 10)** — Phase C screen
//! tasks never edit it (see `src/README-phase-c.md`). The shell is the app's
//! first real [`icon`](forgekit::icon) consumer: each tab carries an
//! [`icons`](forgekit::icons) glyph. The bottom bar branches its
//! Material/Cupertino chrome on the active [`DesignLanguage`] (the settings
//! appearance screen swaps it live).
//!
//! The tab roots fade into each other: the shell's [`navigator`] runs an M3
//! fade-through transition, so selecting a tab (`router.go(tab.route())`)
//! cross-fades the tab roots. Detail pushes (channel/thread/settings) ride the
//! same navigator; per-push slide transitions are a Phase C refinement.

use std::rc::Rc;

use forgekit::{
    AnyView, DesignLanguage, Get, RouterDeepLinks, RwSignal, Set, any, cupertino_tab_bar, icon,
    icons, nav_item, navigation_bar, tab_item,
};

use crate::HuddleState;

/// The four bottom-navigation destinations. Each maps to a top-level route the
/// tab selects via `router.go(..)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    /// Channels + DMs (`/`).
    #[default]
    Home,
    /// Search across channels/messages/people (`/search`).
    Search,
    /// The mentions/activity feed (`/activity`).
    Activity,
    /// The current user + settings entry (`/you`).
    You,
}

impl Tab {
    /// Every tab, in display / nav-bar order.
    pub const ALL: [Tab; 4] = [Tab::Home, Tab::Search, Tab::Activity, Tab::You];

    /// The nav-bar label.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Search => "Search",
            Tab::Activity => "Activity",
            Tab::You => "You",
        }
    }

    /// The route this tab navigates to when selected.
    pub fn route(self) -> &'static str {
        match self {
            Tab::Home => "/",
            Tab::Search => "/search",
            Tab::Activity => "/activity",
            Tab::You => "/you",
        }
    }

    /// The Material Symbols glyph for this tab's nav-bar icon.
    pub fn icon(self) -> forgekit::IconSource {
        match self {
            Tab::Home => icons::HOME,
            Tab::Search => icons::SEARCH,
            Tab::Activity => icons::NOTIFICATIONS,
            Tab::You => icons::PERSON,
        }
    }

    /// This tab's index in [`Tab::ALL`] (the nav bar's `selected` position).
    pub fn index(self) -> usize {
        Tab::ALL
            .iter()
            .position(|t| *t == self)
            .expect("self is always a member of ALL")
    }

    /// The tab at nav-bar index `idx` (falls back to the default on an
    /// out-of-range index).
    pub fn from_index(idx: usize) -> Self {
        Tab::ALL.get(idx).copied().unwrap_or_default()
    }
}

/// The persistent bottom bar: a Material [`navigation_bar`] or a Cupertino
/// [`cupertino_tab_bar`] (per the active `design`), each with a per-tab
/// [`icon`]. Selecting a destination records it in `tab` (the highlight) and
/// drives the router to that tab's route.
pub fn bottom_bar(
    design: DesignLanguage,
    nav: Rc<RouterDeepLinks<HuddleState>>,
    tab: RwSignal<Tab>,
) -> AnyView<HuddleState> {
    let selected = tab.get().index();
    match design {
        DesignLanguage::Material3 => any(navigation_bar::<HuddleState, _>(
            Tab::ALL
                .iter()
                .map(|t| nav_item::<HuddleState>(t.label()).icon(any(icon(t.icon()).size(24.0))))
                .collect(),
            selected,
            move |_s: &mut HuddleState, idx: usize| {
                let t = Tab::from_index(idx);
                tab.set(t);
                nav.router().go(t.route());
            },
        )),
        DesignLanguage::Cupertino => any(cupertino_tab_bar::<HuddleState, _>(
            Tab::ALL
                .iter()
                .map(|t| tab_item::<HuddleState>(t.label()).icon(any(icon(t.icon()).size(24.0))))
                .collect(),
            selected,
            move |_s: &mut HuddleState, idx: usize| {
                let t = Tab::from_index(idx);
                tab.set(t);
                nav.router().go(t.route());
            },
        )),
    }
}
