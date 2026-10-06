//! Refresh indicator: the reference's `RefreshIndicatorPlayground`.
//!
//! # Descoped: `material`/`adaptive` kinds, and the `Trigger` control
//!
//! The reference's `_RefreshKind` has five values (`expressive`,
//! `contained`, `material`, `adaptive`, `noSpinner`); `frust_material::
//! refresh_indicator`'s own module docs scope out the Flutter-native
//! `material`/`adaptive` wraps entirely ("only the two M3E-shaped ones plus
//! `no_spinner`... are ported") — this page's [`RefreshKind`] narrows to the
//! three the crate actually builds
//! ([`RefreshIndicatorView::contained`]/[`RefreshIndicatorView::no_spinner`]
//! plus the expressive default). The reference's `_trigger`
//! (`M3ERefreshTriggerMode.onEdge`/`.anywhere`, a `PlayEnumSegmented`) has no
//! prop on `RefreshIndicatorView` at all — the port's trigger is always the
//! baseline `ScrollView`'s own fixed release threshold (see that module's
//! own docs) — so the control is omitted rather than driving nothing.
//!
//! # Simulated refresh: a hand-rolled timer, no new dependency
//!
//! [`RefreshIndicatorView::refreshing`]'s controlled contract needs the app
//! to flip a prop back to `false` on a *later* rebuild once its async work
//! finishes (that module's own docs). This page simulates the reference's
//! `Future<void>.delayed(const Duration(seconds: 2))` the same way
//! `examples/glyph-catalog/src/pages/appbar.rs`'s connection-banner demo
//! does (its module docs' "Timer-driven demos with no `tokio` dependency"
//! section): `material3-demo` carries no direct `tokio` dependency, so the
//! framework's blessed `tokio::time::sleep` idiom is unavailable here.
//! [`Delay`] is that same minimal one-shot timer future ported verbatim — a
//! background `std::thread::sleep` wakes the polling `frust::spawn_local`
//! task on completion, the identical background-thread-wakes-a-task wiring
//! `frust_reactive::executor`'s composite-waker contract documents for a
//! real tokio timer, so the redraw-on-background-wake path is exercised for
//! real with no new crate. Bridging the async completion back into this
//! page's plain [`Knobs`] needs a *signal* — the one place this file departs
//! from the plain-retained-`Knobs` convention this section's other pages
//! follow, since a `Component`'s own state is only reachable through an
//! event-dispatched callback, never from a task that outlives it; a tracked
//! `frust::RwSignal` write is what notifies the shell to schedule the next
//! frame at all (`docs/ARCHITECTURE.md`'s Rebuild wake flow).
//!
//! # Content: `card_list_items` over 12 rows
//!
//! The reference's `_listChild` (`M3ECardList.builder`, 12 `M3EListItem`
//! rows) is [`refresh_list_content`], over [`frust_material::card_list_items`]/
//! [`frust_material::list_item`] — the same pairing
//! `frust_material::card_list`'s own module docs name for a `ListItem`-backed
//! card list.
//!
//! # Preview frame: a bordered `container`, not a true clip
//!
//! The reference wraps its preview in a `DecoratedBox` border plus a
//! `ClipRRect`; this port keeps the border (`frust::container`'s own
//! `.radius()`/`.border()`) but not the clip — `ContainerView` has no clip
//! builder, and the scrollable content underneath a fixed-height box already
//! stays within its own bounds during ordinary scrolling, so the visual gap
//! is limited to a refresh indicator briefly overshooting the frame's
//! rounded corner during its pull/retract animation (approximating, not
//! reproducing — the same divergence
//! `crate::pages::playground::view::shapes`' clipped-child preview
//! documents).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use frust::{
    AnyView, Component, Get, RwSignal, Set, SizedBox, Stack, Update, View, any, component,
    container, icon, spawn_local,
};

use frust_material::{
    MaterialDimensions, OverlayAnchor, card_list_items, icons, list_item, refresh_indicator,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel, play_preview_card,
    play_snippet, playground_body,
};

// ---------------------------------------------------------------------------
// Delay — a dependency-free one-shot timer future (see the module docs'
// Simulated refresh section). Ported verbatim from
// `examples/glyph-catalog/src/pages/appbar.rs`'s own `Delay`.
// ---------------------------------------------------------------------------

/// A minimal one-shot timer future built on `std::thread`/`Waker`.
struct Delay {
    deadline: Instant,
}

impl Delay {
    fn new(duration: Duration) -> Self {
        Delay {
            deadline: Instant::now() + duration,
        }
    }
}

impl Future for Delay {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let now = Instant::now();
        if now >= self.deadline {
            Poll::Ready(())
        } else {
            let remaining = self.deadline - now;
            let waker = cx.waker().clone();
            std::thread::spawn(move || {
                std::thread::sleep(remaining);
                waker.wake();
            });
            Poll::Pending
        }
    }
}

/// The simulated refresh's duration — the reference's own
/// `Future<void>.delayed(const Duration(seconds: 2))`.
const SIMULATED_REFRESH: Duration = Duration::from_secs(2);

/// The preview's fixed height — the reference's `SizedBox(height: 280)`.
const PREVIEW_HEIGHT: f64 = 280.0;

/// The reference's `_RefreshKind`, narrowed to the three constructors
/// `frust_material::refresh_indicator` actually ports — see the module
/// docs' Descoped section.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RefreshKind {
    Expressive,
    Contained,
    NoSpinner,
}

impl RefreshKind {
    const ALL: [RefreshKind; 3] = [
        RefreshKind::Expressive,
        RefreshKind::Contained,
        RefreshKind::NoSpinner,
    ];

    /// The reference's `kind.name`.
    fn label(self) -> &'static str {
        match self {
            RefreshKind::Expressive => "expressive",
            RefreshKind::Contained => "contained",
            RefreshKind::NoSpinner => "noSpinner",
        }
    }
}

/// This playground's knobs — the reference's
/// `_RefreshIndicatorPlaygroundState`. [`Knobs::menu_anchor`] is the one
/// plain shared field; [`Knobs::refreshing`]/[`Knobs::refresh_count`] are the
/// one place this file needs real signals — see the module docs' Simulated
/// refresh section.
struct Knobs {
    kind: RefreshKind,
    menu_open: bool,
    menu_anchor: OverlayAnchor,
    refreshing: RwSignal<bool>,
    refresh_count: RwSignal<u32>,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            kind: RefreshKind::Expressive,
            menu_open: false,
            menu_anchor: OverlayAnchor::new(),
            refreshing: RwSignal::new(false),
            refresh_count: RwSignal::new(0),
        }
    }
}

/// The controlled refresh lifecycle's start (`RefreshIndicatorView`'s own
/// module docs): flip `refreshing` on immediately, then simulate the
/// reference's async work via [`Delay`] before confirming completion on a
/// later rebuild. See the module docs' Simulated refresh section.
fn on_refresh(state: &mut Knobs) {
    state.refreshing.set(true);
    let refreshing = state.refreshing;
    let refresh_count = state.refresh_count;
    spawn_local(async move {
        Delay::new(SIMULATED_REFRESH).await;
        refresh_count.update(|c| *c += 1);
        refreshing.set(false);
    });
}

/// The scrollable content every kind wraps — the reference's `_listChild`
/// (`M3ECardList.builder`, 12 rows). See the module docs' Content section.
fn refresh_list_content() -> AnyView<Knobs> {
    let items = (1..=12).map(|n| {
        list_item::<Knobs>(format!("Item {n}"))
            .supporting("Pull down to refresh")
            .leading(icon(icons::REFRESH))
    });
    any(card_list_items(items))
}

/// Build the indicator `kind` selects, wired to the shared [`on_refresh`]
/// callback and the current controlled `refreshing` prop — the reference's
/// `_buildIndicator`.
fn build_indicator(kind: RefreshKind, refreshing: bool) -> AnyView<Knobs> {
    match kind {
        RefreshKind::Expressive => {
            any(refresh_indicator(refresh_list_content(), on_refresh).refreshing(refreshing))
        }
        RefreshKind::Contained => any(refresh_indicator(refresh_list_content(), on_refresh)
            .contained()
            .refreshing(refreshing)),
        RefreshKind::NoSpinner => any(refresh_indicator(refresh_list_content(), on_refresh)
            .no_spinner()
            .refreshing(refreshing)),
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    let ctor_suffix = match state.kind {
        RefreshKind::Expressive => String::new(),
        RefreshKind::Contained => "\n    .contained()".to_string(),
        RefreshKind::NoSpinner => "\n    .no_spinner()".to_string(),
    };
    format!(
        "refresh_indicator(content, on_refresh){ctor_suffix}\n    .refreshing(state.refreshing);"
    )
}

/// The playground body for the current knob state: [`playground_body`]'s
/// scrollable content, plus the type picker's anchored panel at the outer
/// `Stack` — see the module docs.
fn body(state: &mut Knobs) -> AnyView<Knobs> {
    let theme = ambient_theme();
    let refreshing = state.refreshing.get();
    let refresh_count = state.refresh_count.get();

    let indicator = build_indicator(state.kind, refreshing);
    let framed = container(SizedBox::<Knobs>(None, Some(PREVIEW_HEIGHT)).child(indicator))
        .radius(MaterialDimensions::RADIUS_LARGE)
        .border(theme.scheme().outline_variant, 1.0);
    let preview = play_preview_card(format!("Pull to refresh (count: {refresh_count})"), framed);
    let snippet = play_snippet("Pull to refresh", snippet_code(state));

    let kind_field = play_enum_menu_field::<Knobs, RefreshKind>(
        "Kind",
        state.kind,
        &RefreshKind::ALL,
        RefreshKind::label,
        &state.menu_anchor,
        state.menu_open,
        |s: &mut Knobs, open: bool| s.menu_open = open,
    );
    let controls = control_panel("Appearance", vec![kind_field]);

    let content = playground_body(vec![preview], vec![snippet], vec![controls]);

    let panel = play_enum_menu_panel::<Knobs, RefreshKind>(
        state.kind,
        &RefreshKind::ALL,
        RefreshKind::label,
        &state.menu_anchor,
        state.menu_open,
        |s: &mut Knobs, open: bool| s.menu_open = open,
        |s: &mut Knobs, next: RefreshKind| s.kind = next,
    );

    any(Stack(vec![content, panel]))
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct RefreshIndicatorPlayground;

impl Component for RefreshIndicatorPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(RefreshIndicatorPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body —
    /// every kind, the type picker's field+panel pair sharing one
    /// `frust_material::OverlayAnchor`, and the controlled `refreshing`/
    /// `refresh_count` signals mid-cycle. [`on_refresh`] itself (which
    /// spawns a real background thread) is deliberately never invoked here —
    /// this sweep only builds views, matching every sibling page's own
    /// construction-only sweep.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for kind in RefreshKind::ALL {
            knobs.kind = kind;
            for menu_open in [false, true] {
                knobs.menu_open = menu_open;
                let _view = body(&mut knobs);
            }
            knobs.menu_open = false;
        }
        knobs.refreshing.set(true);
        knobs.refresh_count.set(3);
        let _view = body(&mut knobs);
        knobs.refreshing.set(false);
        let _view = body(&mut knobs);
    }

    #[test]
    fn the_snippet_names_the_right_constructor_suffix_per_kind() {
        let mut knobs = Knobs::default();
        assert!(!snippet_code(&knobs).contains(".contained()"));
        assert!(!snippet_code(&knobs).contains(".no_spinner()"));

        knobs.kind = RefreshKind::Contained;
        assert!(snippet_code(&knobs).contains(".contained()"));

        knobs.kind = RefreshKind::NoSpinner;
        assert!(snippet_code(&knobs).contains(".no_spinner()"));
    }

    #[test]
    fn refresh_kind_label_matches_the_reference_names() {
        assert_eq!(RefreshKind::Expressive.label(), "expressive");
        assert_eq!(RefreshKind::Contained.label(), "contained");
        assert_eq!(RefreshKind::NoSpinner.label(), "noSpinner");
    }
}
