//! The persistent Huddle shell: the 4-tab bottom navigation
//! (Home / Search / Activity / You) and the shared [`Tab`] enum every tab
//! routes on.
//!
//! This is a hub file shared by every feature (see `src/README-phase-c.md`
//! for the feature-slice convention). The shell is the app's
//! first real [`icon`](frust::icon) consumer: each tab carries an
//! [`icons`](frust::icons) glyph. The bottom bar branches its
//! Material/Cupertino/Glyph chrome on the active [`DesignLanguage`] (the
//! settings appearance screen swaps it live).
//!
//! The tab roots fade into each other: the shell's [`navigator`] runs an M3
//! fade-through transition, so selecting a tab (`router.go(tab.route())`)
//! cross-fades the tab roots. Detail pushes (channel/thread/settings) ride the
//! same navigator; per-push slide transitions are a Phase C refinement.

use std::rc::Rc;

use frust::{
    AnyView, DesignLanguage, Get, RouterDeepLinks, RwSignal, Set, any, cupertino_tab_bar,
    glyph::{glyph_nav_bar, glyph_nav_item_icon},
    icon, icons, nav_item, navigation_bar, safe_area, tab_item,
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
    pub fn icon(self) -> frust::IconSource {
        match self {
            Tab::Home => icons::HOME,
            Tab::Search => icons::SEARCH,
            Tab::Activity => icons::NOTIFICATIONS,
            Tab::You => icons::PERSON,
        }
    }

    /// This tab's Glyph bottom-nav icon (task 28) — standing in for
    /// [`Tab::icon`]'s Material Symbols path on the Glyph design
    /// language, matching the reference build's
    /// `.bottom-nav-glyph` geometric shapes (▣ ◎ ◆ ◉) — drawn as vector
    /// paths, NOT font glyphs: the bundled Glyph fonts carry no Geometric
    /// Shapes coverage, so char rendering rode each platform's system
    /// fallback (fine on Android, tofu boxes on iOS — found on-device
    /// 2026-07-23). Paths render identically everywhere. Memoized in
    /// `LazyLock`s so the per-frame rebuild sees a stable `Arc` identity
    /// (the navbar's structural diff uses `IconData::same`).
    pub fn glyph_icon(self) -> frust::IconData {
        use kurbo::{BezPath, Circle, Rect, Shape};
        use std::sync::LazyLock;
        const TOL: f64 = 0.05;
        /// Outer shape minus inner shape as a NonZero-fill ring.
        fn ring(outer: BezPath, inner: BezPath) -> BezPath {
            let mut p = outer;
            p.extend(inner.reverse_subpaths());
            p
        }
        // All authored against a 10×10 design box.
        static HOME: LazyLock<frust::IconData> = LazyLock::new(|| {
            // ▣ — square outline with a filled inner square.
            let mut p = ring(
                Rect::new(0.5, 0.5, 9.5, 9.5).to_path(TOL),
                Rect::new(1.8, 1.8, 8.2, 8.2).to_path(TOL),
            );
            p.extend(Rect::new(3.2, 3.2, 6.8, 6.8).to_path(TOL));
            frust::IconData::from_path(p, 10.0)
        });
        static SEARCH: LazyLock<frust::IconData> = LazyLock::new(|| {
            // ◎ — two concentric circle rings (bullseye).
            let mut p = ring(
                Circle::new((5.0, 5.0), 4.6).to_path(TOL),
                Circle::new((5.0, 5.0), 3.4).to_path(TOL),
            );
            p.extend(ring(
                Circle::new((5.0, 5.0), 2.2).to_path(TOL),
                Circle::new((5.0, 5.0), 1.2).to_path(TOL),
            ));
            frust::IconData::from_path(p, 10.0)
        });
        static ACTIVITY: LazyLock<frust::IconData> = LazyLock::new(|| {
            // ◆ — filled diamond.
            let mut p = BezPath::new();
            p.move_to((5.0, 0.2));
            p.line_to((9.8, 5.0));
            p.line_to((5.0, 9.8));
            p.line_to((0.2, 5.0));
            p.close_path();
            frust::IconData::from_path(p, 10.0)
        });
        static YOU: LazyLock<frust::IconData> = LazyLock::new(|| {
            // ◉ — circle outline with a filled center dot.
            let mut p = ring(
                Circle::new((5.0, 5.0), 4.6).to_path(TOL),
                Circle::new((5.0, 5.0), 3.4).to_path(TOL),
            );
            p.extend(Circle::new((5.0, 5.0), 2.0).to_path(TOL));
            frust::IconData::from_path(p, 10.0)
        });
        match self {
            Tab::Home => HOME.clone(),
            Tab::Search => SEARCH.clone(),
            Tab::Activity => ACTIVITY.clone(),
            Tab::You => YOU.clone(),
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

/// The persistent bottom bar: a Material [`navigation_bar`], a Cupertino
/// [`cupertino_tab_bar`], or a Glyph `glyph_nav_bar` (per the active
/// `design`) — the Material/Cupertino bars carry a per-tab [`icon`], the
/// Glyph bar a per-tab monospace glyph char ([`Tab::glyph`]). Selecting a
/// destination records it in `tab` (the highlight) and drives the router to
/// that tab's route.
///
/// Wrapped in [`safe_area`] (device-parity task 14, item 1) so the bar's
/// bottom edge clears the system gesture/nav bar — `top` stays unpadded (the
/// bar already sits directly under the tab-shell's own content) while
/// `left`/`right` stay enabled for a landscape display cutout. This is a
/// huddle-side composition, not a `frust-widgets` change: the bar's own
/// background still only fills its un-padded content height (it does not
/// extend a themed fill under the inset the way a native edge-to-edge bar
/// would) — a known v1 gap, see this task's completion summary.
pub fn bottom_bar(
    design: DesignLanguage,
    nav: Rc<RouterDeepLinks<HuddleState>>,
    tab: RwSignal<Tab>,
) -> AnyView<HuddleState> {
    let selected = tab.get().index();
    let bar: AnyView<HuddleState> = match design {
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
        // `glyph_nav_bar`'s (items, selected, on_select(index)) shape is the
        // same controlled-index contract `navigation_bar`/`cupertino_tab_bar`
        // use above, so it fits this shell's tab model directly (task 28).
        DesignLanguage::Glyph => any(glyph_nav_bar::<HuddleState, _>(
            Tab::ALL
                .iter()
                .map(|t| glyph_nav_item_icon(t.glyph_icon(), t.label()))
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
    };
    any(safe_area(bar).top(false))
}
