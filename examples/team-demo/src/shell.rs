//! The persistent showcase shell: the top app bar and the bottom navigation,
//! plus the two shared enums the whole app routes on ([`Tab`],
//! [`TransitionChoice`]).
//!
//! Both bars are built in [`ShellApp::build`](crate::ShellApp) (where the live
//! state is in scope), so they read state directly and capture an
//! `Rc<RouterDeepLinks>` handle into their navigation closures rather than a
//! signal. They branch their Material/Cupertino chrome on the active
//! [`DesignLanguage`] — the same `examples/catalog` scaffold pattern. This is
//! a finalized shared file: wave-2 tasks never edit it (see the crate docs'
//! placeholder contract).

use std::rc::Rc;

use forgekit::{
    AnyView, Button, DesignLanguage, Get, PageTransition, RouterDeepLinks, RwSignal, Set,
    TransitionSpec, any, app_bar, cupertino_nav_bar, cupertino_tab_bar, nav_item, navigation_bar,
    tab_item,
};

use crate::ShellState;

/// The four bottom-navigation destinations. Each maps to a top-level route the
/// tab selects via `router.go(..)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    /// The async team roster (`/`).
    #[default]
    Team,
    /// The widget catalog (`/widgets/controls`, the first widgets sub-screen).
    Widgets,
    /// The theme/motion/notes/nav menu (`/showcase`).
    Showcase,
    /// Design-language + brightness settings (`/settings`).
    Settings,
}

impl Tab {
    /// Every tab, in display / nav-bar order.
    pub const ALL: [Tab; 4] = [Tab::Team, Tab::Widgets, Tab::Showcase, Tab::Settings];

    /// The nav-bar label.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Team => "Team",
            Tab::Widgets => "Widgets",
            Tab::Showcase => "Showcase",
            Tab::Settings => "Settings",
        }
    }

    /// The route this tab navigates to when selected.
    pub fn route(self) -> &'static str {
        match self {
            Tab::Team => "/",
            Tab::Widgets => "/widgets/controls",
            Tab::Showcase => "/showcase",
            Tab::Settings => "/settings",
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

/// Which page-transition preset the navigator currently applies, driven live by
/// the nav playground's switcher (see [`crate::screens::nav_playground`]).
/// `IosPush` is also what arms the interactive left-edge swipe-back
/// (`NavigatorView::pop_swipe`'s default: on for `IosPush`, off otherwise) —
/// the same preset vocabulary `examples/navdemo` exposes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TransitionChoice {
    /// Instant (no animation).
    #[default]
    None,
    /// M3 shared-axis-X.
    M3SharedAxis,
    /// M3 fade-through.
    M3FadeThrough,
    /// iOS push (arms swipe-back).
    Ios,
}

impl TransitionChoice {
    /// A human-readable label for the current preset.
    pub fn label(self) -> &'static str {
        match self {
            TransitionChoice::None => "None (instant)",
            TransitionChoice::M3SharedAxis => "M3 shared-axis-X",
            TransitionChoice::M3FadeThrough => "M3 fade-through",
            TransitionChoice::Ios => "iOS push (swipe-back armed)",
        }
    }

    /// The [`TransitionSpec`] the navigator drives for this preset.
    pub fn to_spec(self) -> TransitionSpec {
        match self {
            TransitionChoice::None => TransitionSpec::NONE,
            TransitionChoice::M3SharedAxis => {
                TransitionSpec::duration(PageTransition::M3SharedAxisX)
            }
            TransitionChoice::M3FadeThrough => {
                TransitionSpec::duration(PageTransition::M3FadeThrough)
            }
            TransitionChoice::Ios => TransitionSpec::duration(PageTransition::IosPush),
        }
    }
}

/// The persistent top bar: a Material [`app_bar`] or a Cupertino
/// [`cupertino_nav_bar`] (per the active `design`), each with a trailing
/// "Profile" button — the avatar affordance — that pushes `/profile`.
pub fn top_bar(
    design: DesignLanguage,
    nav: Rc<RouterDeepLinks<ShellState>>,
) -> AnyView<ShellState> {
    match design {
        DesignLanguage::Material3 => {
            any(
                app_bar::<ShellState>("ForgeKit Showcase").actions(vec![any(Button(
                    "Profile",
                    move |_s: &mut ShellState| {
                        nav.router().push("/profile");
                    },
                ))]),
            )
        }
        DesignLanguage::Cupertino => any(cupertino_nav_bar::<ShellState>("ForgeKit Showcase")
            .trailing(any(Button("Profile", move |_s: &mut ShellState| {
                nav.router().push("/profile");
            })))),
    }
}

/// The persistent bottom bar: a Material [`navigation_bar`] or a Cupertino
/// [`cupertino_tab_bar`] (per the active `design`). Selecting a destination
/// records it in `tab` (the highlight) and drives the router to that tab's
/// route.
pub fn bottom_bar(
    design: DesignLanguage,
    nav: Rc<RouterDeepLinks<ShellState>>,
    tab: RwSignal<Tab>,
) -> AnyView<ShellState> {
    let selected = tab.get().index();
    match design {
        DesignLanguage::Material3 => any(navigation_bar::<ShellState, _>(
            Tab::ALL
                .iter()
                .map(|t| nav_item::<ShellState>(t.label()))
                .collect(),
            selected,
            move |_s: &mut ShellState, idx: usize| {
                let t = Tab::from_index(idx);
                tab.set(t);
                nav.router().go(t.route());
            },
        )),
        DesignLanguage::Cupertino => any(cupertino_tab_bar::<ShellState, _>(
            Tab::ALL
                .iter()
                .map(|t| tab_item::<ShellState>(t.label()))
                .collect(),
            selected,
            move |_s: &mut ShellState, idx: usize| {
                let t = Tab::from_index(idx);
                tab.set(t);
                nav.router().go(t.route());
            },
        )),
    }
}
