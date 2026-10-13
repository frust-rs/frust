//! Tabs section: the Android/Material tab-back convention, device-gateable.
//!
//! Three tabs (`Home`, `B`, `C`), each with its **own** [`NavigatorController`]
//! rendered through [`frust::navigator`] (so back auto-wires), stacked in a
//! fixed-height [`IndexedStack`]. A [`BackFallback`] wrapped around the stack
//! with [`back_fallback`] implements "back at a non-home tab root returns to
//! Home; back at the Home root exits":
//!
//! 1. A tab whose navigator is deeper than its root pops first (navigators
//!    outrank a fallback).
//! 2. At a non-home tab root, the fallback claims the press and selects Home.
//! 3. At the Home root nothing claims it, so the platform exits the app.
//!
//! Each tab root has a **Push** button that stacks numbered detail pages; tab
//! B's root also carries a single-line text field (the focus/IME leg of the
//! device gate). A status line prints the selected tab and every tab's depth so
//! the gate can read state from a screenshot.
//!
//! The tab bodies sit in a [`SizedBox`] of [`TAB_BODY_H`] because the shell
//! wraps every page in a vertical `scroll_view`: a navigator needs bounded
//! height.
//!
//! The fallback is built once in [`TabsState::new`]. Its wrapper is mounted
//! only while this section is on screen, so switching to another section
//! stops it claiming presses (see `frust::BackFallback`'s liveness contract).

use frust::{
    BackFallback, ButtonStyle, Color, EdgeInsets, Either, Get, GetUntracked, IndexedStack,
    NavigatorController, NavigatorView, Padding, RouteObserver, RwSignal, Set, SizedBox, Theme,
    View, back_fallback, button, column, navigator, row, text, text_input, use_context,
};

use crate::PlaygroundState;

/// The home tab's index: back at any other tab's root returns here.
pub const HOME: usize = 0;

/// The tab labels, index-aligned with [`TabsState::navs`].
pub const TAB_LABELS: [&str; 3] = ["Home", "B", "C"];

/// The fixed height of the tab body: the page sits in the shell's vertical
/// scroll view, so the tab navigators need an explicit bounded height.
const TAB_BODY_H: f64 = 420.0;

/// The "Tabs" section's state, owned by [`PlaygroundState`]. Every field is a
/// clone-shares-one-handle value, so cloning the playground state shares it.
#[derive(Clone)]
pub struct TabsState {
    /// The selected tab index.
    pub selected: RwSignal<usize>,
    /// One navigator per tab, index-aligned with [`TAB_LABELS`].
    pub navs: [NavigatorController<PlaygroundState>; 3],
    /// One reactive route observer per tab, feeding the status line's depths.
    pub routes: [RouteObserver; 3],
    /// Tab B's text field contents.
    pub b_text: RwSignal<String>,
    /// Back at a non-home tab root returns to [`HOME`]; constructed once here.
    pub home_fallback: BackFallback,
}

impl TabsState {
    /// Fresh state: Home selected, three empty navigators, and the home
    /// fallback (interest: off Home with the selected tab at its root).
    pub fn new() -> Self {
        let selected = RwSignal::new(HOME);
        let navs = [
            NavigatorController::new(),
            NavigatorController::new(),
            NavigatorController::new(),
        ];
        let probe = navs.clone();
        let home_fallback = BackFallback::new(
            move || {
                let sel = selected.get_untracked();
                sel != HOME && probe.get(sel).is_some_and(|nav| nav.depth() <= 1)
            },
            move || selected.set(HOME),
        );
        TabsState {
            selected,
            navs,
            routes: [
                RouteObserver::new(),
                RouteObserver::new(),
                RouteObserver::new(),
            ],
            b_text: RwSignal::new(String::new()),
            home_fallback,
        }
    }
}

impl Default for TabsState {
    fn default() -> Self {
        Self::new()
    }
}

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

/// Push a numbered detail page (`level` >= 2) onto `nav`.
fn push_detail(nav: &NavigatorController<PlaygroundState>, tab: usize, level: usize) {
    let host = nav.clone();
    nav.push(move || detail_page(host.clone(), tab, level));
}

/// A "Push" button that stacks the next detail page on `nav`.
fn push_button(
    nav: &NavigatorController<PlaygroundState>,
    tab: usize,
    next_level: usize,
) -> impl View<PlaygroundState> + use<> {
    let nav = nav.clone();
    button("Push", move |_state: &mut PlaygroundState| {
        push_detail(&nav, tab, next_level);
    })
    .style(ButtonStyle::Primary)
    .small()
}

/// A pushed detail page showing its depth, with its own "Push".
fn detail_page(
    nav: NavigatorController<PlaygroundState>,
    tab: usize,
    level: usize,
) -> impl View<PlaygroundState> {
    Padding(
        EdgeInsets::all(12.0),
        column()
            .child(
                text(format!("{} detail", TAB_LABELS[tab]))
                    .size(13.0)
                    .color(accent()),
            )
            .child(text(format!("depth {level}")).size(12.0))
            .child(SizedBox(None, Some(8.0)))
            .child(push_button(&nav, tab, level + 1)),
    )
}

/// A tab's root page. Tab B additionally carries a single-line text field.
fn root_page(
    nav: NavigatorController<PlaygroundState>,
    tab: usize,
    b_text: RwSignal<String>,
) -> impl View<PlaygroundState> {
    let field = if tab == 1 {
        Either::Left(
            text_input(b_text.get(), move |_state: &mut PlaygroundState, value| {
                b_text.set(value);
            })
            .placeholder("Type here (focus / IME)"),
        )
    } else {
        Either::Right(SizedBox::<PlaygroundState>(None, Some(0.0)))
    };
    Padding(
        EdgeInsets::all(12.0),
        column()
            .child(
                text(format!("{} root", TAB_LABELS[tab]))
                    .size(13.0)
                    .color(accent()),
            )
            .child(text("depth 1").size(12.0))
            .child(SizedBox(None, Some(8.0)))
            .child(push_button(&nav, tab, 2))
            .child(SizedBox(None, Some(8.0)))
            .child(field),
    )
}

/// Tab `tab`'s navigator, observed for the status line's depth.
fn tab_navigator(tabs: &TabsState, tab: usize) -> NavigatorView<PlaygroundState> {
    let nav = tabs.navs[tab].clone();
    let b_text = tabs.b_text;
    let host = nav.clone();
    tabs.routes[tab].observe(navigator(&nav, move || {
        root_page(host.clone(), tab, b_text)
    }))
}

/// The row of tab-selector buttons; the selected tab is drawn Primary.
fn tab_buttons(selected: usize, signal: RwSignal<usize>) -> impl View<PlaygroundState> + use<> {
    let tab_button = move |tab: usize| {
        button(TAB_LABELS[tab], move |_state: &mut PlaygroundState| {
            signal.set(tab);
        })
        .style(if tab == selected {
            ButtonStyle::Primary
        } else {
            ButtonStyle::Secondary
        })
        .small()
    };
    row()
        .child(tab_button(0))
        .child(SizedBox(Some(8.0), None))
        .child(tab_button(1))
        .child(SizedBox(Some(8.0), None))
        .child(tab_button(2))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let tabs = &state.tabs;
    let selected = tabs.selected.get();
    let status = format!(
        "selected: {}   depth Home {} / B {} / C {}",
        TAB_LABELS[selected.min(TAB_LABELS.len() - 1)],
        tabs.routes[0].depth(),
        tabs.routes[1].depth(),
        tabs.routes[2].depth(),
    );

    let stack = IndexedStack(
        Some(selected),
        (
            tab_navigator(tabs, 0),
            tab_navigator(tabs, 1),
            tab_navigator(tabs, 2),
        ),
    );
    let body = SizedBox::<PlaygroundState>(None, Some(TAB_BODY_H))
        .child(back_fallback(&tabs.home_fallback, stack));

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(text("Tabs").size(13.0).color(accent()))
            .child(
                text(
                    "Three tabs, each with its own navigator. Push pages in a tab, then \
                     press back: a deeper tab pops first; at a non-Home tab root back \
                     returns to Home; at the Home root it exits the app.",
                )
                .size(11.0)
                .color(muted()),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(tab_buttons(selected, tabs.selected))
            .child(SizedBox(None, Some(8.0)))
            .child(body)
            .child(SizedBox(None, Some(8.0)))
            .child(text(status).size(12.0)),
    )
}
