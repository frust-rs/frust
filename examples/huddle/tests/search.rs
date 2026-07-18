//! Search screen tests: typing filters all three sections (and caps message
//! hits), the clear button resets to the hint state, a non-matching query
//! shows "no results", and a result row navigates.
//!
//! Every test that touches process-global reactive state takes the
//! [`support::serial`] lock first, mirroring `tests/shell.rs`'s own
//! convention (see that file's harness docs).
//!
//! Row-tap coordinates below are exact, not measured guesses: the search
//! field row and each section header are forced to a fixed height via
//! `SizedBox` (`screens/search.rs`'s private `FIELD_HEIGHT`/
//! `SECTION_HEADER_HEIGHT` constants, mirrored here as the same magic
//! numbers since `screens` is a private module unreachable from this
//! integration-test crate), and a two-line result row's `TWO_LINE_HEIGHT`
//! (72px, re-exported from `forgekit-widgets`) is a fixed, non-font-metric-
//! dependent constant — so `AppBar(64) + field(56) + header(32) +
//! half-row(36) = 188` is fully deterministic, the same style of
//! hand-computed offset `tests/shell.rs`'s bottom-bar test (`bar_y = 568.0`)
//! already relies on.

use forgekit::{AnyView, Component, GetUntracked};
use forgekit_core::RenderRoot;
use forgekit_text::TextContext;
use kurbo::Point;

use huddle::features::search::{MAX_MESSAGE_HITS, SearchController};
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{RecScene, W, char_key, frame, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// AppBar height (a private const in `forgekit-widgets::material::appbar`,
/// mirrored here — the same precedent `tests/shell.rs`'s `bar_y` uses).
const APP_BAR_HEIGHT: f64 = 64.0;
/// Mirrors `screens/search.rs`'s private `FIELD_HEIGHT`.
const FIELD_HEIGHT: f64 = 56.0;
/// Mirrors `screens/search.rs`'s private `SECTION_HEADER_HEIGHT`.
const SECTION_HEADER_HEIGHT: f64 = 32.0;
/// Half of `forgekit::TWO_LINE_HEIGHT` (a two-line row's supporting text
/// promotes every search result row to that height).
const HALF_TWO_LINE_ROW: f64 = forgekit::TWO_LINE_HEIGHT / 2.0;

/// The bottom navigation bar's Search tab slot (see `tests/shell.rs`'s own
/// `slot_centers`/`bar_y`): tapping here switches the active tab to Search.
const SEARCH_TAB: Point = Point::new(300.0, 568.0);

/// A point safely inside the search field's row, focusing it on tap.
fn field_point() -> Point {
    Point::new(400.0, APP_BAR_HEIGHT + FIELD_HEIGHT / 2.0)
}

/// A point at the clear (`icons::CLOSE`) button's trailing edge, only present
/// once the query is non-empty.
fn clear_button_point() -> Point {
    Point::new(W - 10.0, APP_BAR_HEIGHT + FIELD_HEIGHT / 2.0)
}

/// The center of the first result row (right after the field + one section
/// header) — see the module docs' coordinate derivation.
fn first_result_row_point() -> Point {
    Point::new(
        400.0,
        APP_BAR_HEIGHT + FIELD_HEIGHT + SECTION_HEADER_HEIGHT + HALF_TWO_LINE_ROW,
    )
}

/// Types `text` into the currently-focused field, one `char_key` per
/// character (mirroring a real keystroke stream).
fn type_text(root: &mut Root, state: &mut HuddleState, text: &str) {
    for c in text.chars() {
        root.event(state, &char_key(&c.to_string()));
    }
}

/// Switches the active tab to Search via a real bottom-bar tap.
fn goto_search(root: &mut Root, state: &mut HuddleState) {
    tap(root, state, SEARCH_TAB);
}

/// Builds the app, renders the first frame, and switches to the Search tab —
/// the common setup every test below starts from.
fn boot_to_search() -> (Root, HuddleState, TextContext) {
    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    frame(&mut root, &mut logic, &mut state, &mut tcx);
    goto_search(&mut root, &mut state);
    frame(&mut root, &mut logic, &mut state, &mut tcx);

    (root, state, tcx)
}

fn rebuild(root: &mut Root, state: &mut HuddleState, tcx: &mut TextContext) -> RecScene {
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    frame(root, &mut logic, state, tcx)
}

#[test]
fn typing_filters_all_three_sections_and_caps_messages() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();

    tap(&mut root, &mut state, field_point());
    type_text(&mut root, &mut state, "ada");

    let controller = SearchController::instance();
    assert_eq!(controller.query.get_untracked(), "ada");

    let results = controller.results.get_untracked();
    assert_eq!(results.channels.len(), 0, "no channel is named \"ada\"");
    assert_eq!(results.users.len(), 1, "exactly Ada Lovelace matches");
    assert_eq!(
        results.messages.len(),
        MAX_MESSAGE_HITS,
        "23 raw \"ada\" message hits (3 authored + 20 firehose) cap to MAX_MESSAGE_HITS"
    );

    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the filtered results render");
}

#[test]
fn clear_button_resets_the_query_and_results() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();

    tap(&mut root, &mut state, field_point());
    type_text(&mut root, &mut state, "leadership");

    let controller = SearchController::instance();
    assert_eq!(controller.query.get_untracked(), "leadership");
    assert!(!controller.results.get_untracked().is_empty());

    // A fresh frame renders the clear button (query non-empty) at the field
    // row's trailing edge.
    rebuild(&mut root, &mut state, &mut tcx);
    tap(&mut root, &mut state, clear_button_point());

    assert_eq!(
        controller.query.get_untracked(),
        "",
        "tapping the clear button resets the query"
    );
    assert!(
        controller.results.get_untracked().is_empty(),
        "results clear along with the query"
    );

    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the empty-query hint renders");
}

#[test]
fn a_non_matching_query_shows_no_results() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();

    tap(&mut root, &mut state, field_point());
    type_text(&mut root, &mut state, "zzzznotarealquery");

    let controller = SearchController::instance();
    assert!(
        controller.results.get_untracked().is_empty(),
        "a non-matching query yields no results"
    );

    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the no-results message renders");
}

#[test]
fn a_channel_result_row_navigates_to_its_channel_feed() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();

    tap(&mut root, &mut state, field_point());
    type_text(&mut root, &mut state, "leadership");

    let controller = SearchController::instance();
    let results = controller.results.get_untracked();
    assert_eq!(
        results.channels.len(),
        1,
        "exactly the \"leadership\" channel matches"
    );
    assert!(results.users.is_empty());
    assert!(results.messages.is_empty());

    let before = rebuild(&mut root, &mut state, &mut tcx);
    assert!(before.glyph_runs > 0);

    tap(&mut root, &mut state, first_result_row_point());

    let after = rebuild(&mut root, &mut state, &mut tcx);
    assert!(root.root_id().is_some(), "navigating rebuilds cleanly");
    assert_ne!(
        after.glyph_runs, before.glyph_runs,
        "tapping the row navigated away from Search (the paint output changed)"
    );

    // Round trip: popping back restores the same Search render.
    state.nav.router().controller().pop();
    let restored = rebuild(&mut root, &mut state, &mut tcx);
    assert_eq!(
        restored.glyph_runs, before.glyph_runs,
        "popping back returns to the same Search render"
    );
}
