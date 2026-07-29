//! Search screen tests: typing filters all three sections (and caps message
//! hits), the clear button resets to the hint state, a non-matching query
//! shows "no results", and a result row navigates.
//!
//! Every test that touches process-global reactive state takes the
//! [`support::serial`] lock first, mirroring `tests/shell.rs`'s own
//! convention (see that file's harness docs).
//!
//! The field-focus tap and each section/row-tap coordinate below are forced to
//! fixed heights via `SizedBox` (`screens/search.rs`'s private `FIELD_HEIGHT`/
//! `SECTION_HEADER_HEIGHT` constants, mirrored here as the same magic numbers
//! since `screens` is a private module unreachable from this integration-test
//! crate), and a two-line result row's `TWO_LINE_HEIGHT` (72px, re-exported
//! from `frust-widgets`) is a fixed, non-font-metric-dependent constant —
//! so `AppBar(64) + field(56) + header(32) + half-row(36) = 188` is fully
//! deterministic, the same style of hand-computed offset `tests/shell.rs`'s
//! bottom-bar test (`bar_y = 568.0`) already relies on. The one exception is
//! the *clear* button: the close icon paints no locatable rect and sits at the
//! field row's trailing edge, so its tap point is derived empirically from the
//! painted field chrome (see [`clear_button_point`]).
//!
//! # Settling the boot/tab transition before typing
//!
//! The shell's navigator runs an `M3FadeThrough` cross-fade at boot and again
//! when a tab is selected. The shared [`support::frame`] helper paints at
//! [`FrameTime::ZERO`], which freezes that transition forever — and while a
//! transition is in flight the navigator suppresses ALL page input, so a field
//! tap (and therefore every keystroke) dispatched mid-transition never reaches
//! the search page. [`boot_to_search`] therefore advances the paint clock past
//! the transition ([`settle_transitions`], the same fix `tests/home.rs` uses
//! for its swipe gesture) before any test drives the field. The bottom-bar tab
//! tap itself is shell chrome outside the navigator's page-input region, so it
//! still lands during the frozen transition (as `tests/shell.rs` relies on).

use std::any::Any;

use frust::{AnyView, Component, GetUntracked, Set};
use frust_core::{FrameTime, NamedKey, RenderRoot};
use frust_text::TextContext;
use kurbo::{Point, Size};

use huddle::features::search::{MAX_MESSAGE_HITS, SearchController};
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{H, RecScene, W, char_key, frame, named_key, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// AppBar height (a private const in `frust-widgets::material::appbar`,
/// mirrored here — the same precedent `tests/shell.rs`'s `bar_y` uses).
const APP_BAR_HEIGHT: f64 = 64.0;
/// Mirrors `screens/search.rs`'s private `FIELD_HEIGHT`.
const FIELD_HEIGHT: f64 = 56.0;
/// Mirrors `screens/search.rs`'s private `SECTION_HEADER_HEIGHT`.
const SECTION_HEADER_HEIGHT: f64 = 32.0;
/// Half of `frust::TWO_LINE_HEIGHT` (a two-line row's supporting text
/// promotes every search result row to that height).
const HALF_TWO_LINE_ROW: f64 = frust::TWO_LINE_HEIGHT / 2.0;

/// The bottom navigation bar's Search tab slot (see `tests/shell.rs`'s own
/// `slot_centers`/`bar_y`): tapping here switches the active tab to Search.
const SEARCH_TAB: Point = Point::new(300.0, 568.0);

/// A point safely inside the search field's row, focusing it on tap.
fn field_point() -> Point {
    Point::new(400.0, APP_BAR_HEIGHT + FIELD_HEIGHT / 2.0)
}

/// The painted search-field chrome: the widest recorded rounded rect. It spans
/// nearly the full window width, dwarfing the bottom-bar item chrome, so
/// `max_by` width isolates it regardless of whether the clear button is present
/// (which shrinks the field to make room for the trailing icon).
fn field_chrome(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
        .expect("the search field paints its chrome (a rounded rect)")
}

/// The clear (`icons::CLOSE`) button center, present once the query is
/// non-empty. The icon is `inflexible` immediately trailing the `flexible`
/// field, so it sits just past the field chrome's right edge at the field's
/// vertical center — derived from the paint recording because the icon itself
/// paints only a glyph run, no locatable rect.
fn clear_button_point(scene: &RecScene) -> Point {
    let (origin, size) = field_chrome(scene);
    Point::new(origin.x + size.width + 9.0, origin.y + size.height / 2.0)
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

/// Rebuild + layout + paint like [`support::frame`], but painting at `t_ms` on a
/// caller-advanced clock instead of the shared harness's pinned
/// [`FrameTime::ZERO`]. Returns the recorded scene and whether the paint asked
/// for another frame (an animation — such as a page transition — is still
/// running). Mirrors `tests/home.rs`'s helper of the same name.
fn frame_at(
    root: &mut Root,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    root.rebuild(&mut logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

/// Advance the paint clock until no animation asks for another frame, settling
/// the boot/tab-switch (or push/pop) cross-fade so page input flows again and
/// only the destination page renders. Returns the settled scene (see the
/// module docs, and `tests/home.rs`'s helper of the same name).
fn settle_transitions(root: &mut Root, state: &mut HuddleState, tcx: &mut TextContext) -> RecScene {
    let mut t_ms = 50u64;
    loop {
        t_ms += 16;
        let (scene, needs_frame) = frame_at(root, state, tcx, t_ms);
        if !needs_frame {
            return scene;
        }
        assert!(
            t_ms < 50 + 16 * 300,
            "the boot/tab-switch entrance transition never settled"
        );
    }
}

/// Builds the app, renders the first frame, switches to the Search tab, and
/// settles the ensuing cross-fade (see the module docs) — the common setup
/// every test below starts from, leaving the Search page live and accepting
/// input.
fn boot_to_search() -> (Root, HuddleState, TextContext) {
    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    frame(&mut root, &mut logic, &mut state, &mut tcx);
    goto_search(&mut root, &mut state);
    settle_transitions(&mut root, &mut state, &mut tcx);

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
    // row's trailing edge; locate it from the paint recording and tap it.
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    tap(&mut root, &mut state, clear_button_point(&scene));

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

    // Settle the push cross-fade: the tapped row navigated to the channel feed,
    // whose settled render differs from Search's (proving the tap navigated,
    // not merely that a mid-transition frame paints both pages).
    let after = settle_transitions(&mut root, &mut state, &mut tcx);
    assert!(root.root_id().is_some(), "navigating rebuilds cleanly");
    assert_ne!(
        after.glyph_runs, before.glyph_runs,
        "tapping the row navigated away from Search (the destination render differs)"
    );

    // Round trip: popping back — and settling the pop cross-fade — restores the
    // same Search render.
    state.nav.router().controller().pop();
    let restored = settle_transitions(&mut root, &mut state, &mut tcx);
    assert_eq!(
        restored.glyph_runs, before.glyph_runs,
        "popping back returns to the same Search render"
    );
}

/// Regression for the search-structure restructuring (and an
/// app-tree integration check for a sibling focus-preservation fix):
/// typing a second character — which narrows/changes the matched result set
/// and therefore `results_container`'s internal content — must not disturb
/// the search field's own focus/IME state.
///
/// The query is seeded to one character directly (rather than via a first
/// real keystroke) and settled with its own rebuild *before* this test's
/// typing begins: unlike the mobile shells' continuous per-frame loop, this
/// harness only rebuilds when explicitly told to, and a real per-keystroke
/// rebuild cadence would otherwise also exercise the search field's own
/// separate empty-query/non-empty-query row swap (the clear button
/// appearing swaps `search_field`'s row from a bare `TextInput` to a
/// `FlexView` wrapping one — a genuine `AnyView` type change at that single
/// child, correctly focus-clearing by the same identity-change contract
/// `frust-widgets`' `rebuild_child_clears_active_on_type_swap` unit test
/// documents at the widget level). That transition is orthogonal to (and out
/// of scope for) this task, which is scoped to `results_container`; seeding
/// the query already non-empty isolates the scenario this task actually
/// fixes — a query edit changing `results_container`'s content while the
/// field itself never changes concrete type.
#[test]
fn typing_a_second_character_keeps_the_field_focused() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();

    let controller = SearchController::instance();
    controller.query.set("a".to_string());
    let _ = rebuild(&mut root, &mut state, &mut tcx);

    tap(&mut root, &mut state, field_point());
    assert!(root.is_focus_active(), "tapping the field claims focus");
    assert!(
        root.ime_state().is_some(),
        "a focused TextInput publishes an IME surface"
    );

    // Typing a second character narrows the matched result set, changing
    // results_container's internal content (e.g. fewer/different rows, or a
    // transition into the no-results view) — a structural change confined to
    // index 1 of the outer Column that must leave the field at index 0 (and
    // its recorded focus/IME path) untouched.
    type_text(&mut root, &mut state, "d");
    let scene = rebuild(&mut root, &mut state, &mut tcx);

    assert_eq!(
        controller.query.get_untracked(),
        "ad",
        "the second keystroke reached the still-focused field"
    );
    assert!(
        root.is_focus_active(),
        "the field stays focused after typing a second character"
    );
    assert!(
        root.ime_state().is_some(),
        "the field's IME surface stays published after typing a second character \
         (across the results-list change)"
    );
    assert!(scene.glyph_runs > 0);
}

/// Regression test: `search_field`'s own row —
/// not `results_container` — must never itself change concrete type across
/// the empty/non-empty query boundary (see below for the mechanism that
/// first surfaced this defect). Unlike
/// [`typing_a_second_character_keeps_the_field_focused`] above (which
/// deliberately seeds the query non-empty first, precisely to dodge this
/// exact bug), this test drives the FIRST keystroke from an actually empty
/// query and settles a rebuild immediately afterward — the real per-frame
/// mobile/desktop pipeline's cadence that exposed the defect — before typing
/// a second character. Before the fix, that intervening rebuild would swap
/// `search_field`'s row from a bare `TextInput` to a `FlexView` wrapper (a
/// genuine `AnyView` concrete-type change at `Padding`'s single `ChildPod`),
/// tearing the field down and dropping its recorded focus/IME path — silently
/// swallowing this test's second keystroke.
#[test]
fn typing_across_the_empty_to_nonempty_query_boundary_keeps_the_field_focused() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();
    let controller = SearchController::instance();
    assert_eq!(controller.query.get_untracked(), "");

    tap(&mut root, &mut state, field_point());
    assert!(root.is_focus_active(), "tapping the field claims focus");

    // First keystroke: the empty -> non-empty crossing, immediately settled
    // with a rebuild (the cadence that exposes the pre-fix defect).
    type_text(&mut root, &mut state, "a");
    let _ = rebuild(&mut root, &mut state, &mut tcx);
    assert_eq!(controller.query.get_untracked(), "a");
    assert!(
        root.is_focus_active(),
        "the field stays focused across the empty->non-empty boundary crossing"
    );
    assert!(
        root.ime_state().is_some(),
        "the field's IME surface stays published across the boundary crossing"
    );

    // Second keystroke only lands if the field is still the same focused
    // widget after the boundary-crossing rebuild above.
    type_text(&mut root, &mut state, "d");
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert_eq!(
        controller.query.get_untracked(),
        "ad",
        "the second keystroke reached the still-focused field"
    );
    assert!(
        root.is_focus_active(),
        "the field is still focused after the second keystroke"
    );
    assert!(root.ime_state().is_some());
    assert!(scene.glyph_runs > 0);
}

/// Regression for `12b-search-field-stable-row`'s reverse crossing: clearing
/// a non-empty query back to empty must not drop the field's focus/IME state
/// either, the same `search_field`-row-stability mechanism working backwards.
///
/// The reverse crossing is driven by a real `Backspace` key event, not by
/// tapping the clear-button icon: tapping ANY sibling within `search_field`'s
/// multi-child `FlexView` row always blurs the field via the unrelated
/// (and correct) blur-on-outside-tap convention (`route_event`'s
/// `kept_focus` bookkeeping in `frust-widgets::lib`) — true both before
/// and after this fix, since the row was already this same `FlexView` shape
/// once non-empty even pre-fix. Driving the clear via `Backspace` instead
/// isolates the row-type-stability mechanism this task actually fixes from
/// that unrelated, expected blur. The clear-button TAP path itself is
/// checked separately below, for its own (focus-independent) behavior:
/// tapping it still resets the query.
#[test]
fn backspacing_across_the_nonempty_to_empty_query_boundary_keeps_the_field_focused() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();
    let controller = SearchController::instance();

    tap(&mut root, &mut state, field_point());
    type_text(&mut root, &mut state, "z");
    let _ = rebuild(&mut root, &mut state, &mut tcx);
    assert_eq!(controller.query.get_untracked(), "z");
    assert!(root.is_focus_active());

    // Backspace deletes the sole character, crossing non-empty -> empty.
    // Before the fix this swapped search_field's row from the FlexView
    // wrapper back to a bare TextInput -- the same AnyView type change as the
    // forward direction, in reverse.
    root.event(&mut state, &named_key(NamedKey::Backspace));
    let _ = rebuild(&mut root, &mut state, &mut tcx);
    assert_eq!(
        controller.query.get_untracked(),
        "",
        "backspace clears the last character back to an empty query"
    );
    assert!(
        root.is_focus_active(),
        "the field stays focused across the non-empty->empty boundary crossing"
    );
    assert!(
        root.ime_state().is_some(),
        "the field's IME surface stays published across the reverse boundary crossing"
    );

    // A further keystroke lands: focus truly held through the reverse crossing.
    type_text(&mut root, &mut state, "q");
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert_eq!(controller.query.get_untracked(), "q");
    assert!(root.is_focus_active());
    assert!(scene.glyph_runs > 0);
}

/// The clear button (tap, not `Backspace`) still resets the query and
/// results after `search_field`'s row became a permanently-present `FlexView`
/// (see `12b-search-field-stable-row`) — a basic regression check that the
/// always-rendered (rather than conditionally-rendered) clear-button slot
/// still wires its tap through correctly. Focus is deliberately not asserted
/// here (see `backspacing_across_the_nonempty_to_empty_query_boundary_keeps_the_field_focused`'s
/// doc comment for why a tap on this sibling always blurs the field,
/// independent of this fix).
#[test]
fn the_clear_button_still_resets_the_query_after_the_row_stability_fix() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();
    let controller = SearchController::instance();

    tap(&mut root, &mut state, field_point());
    type_text(&mut root, &mut state, "leadership");
    assert_eq!(controller.query.get_untracked(), "leadership");

    let scene = rebuild(&mut root, &mut state, &mut tcx);
    tap(&mut root, &mut state, clear_button_point(&scene));

    assert_eq!(
        controller.query.get_untracked(),
        "",
        "tapping the clear button still resets the query"
    );
    assert!(controller.results.get_untracked().is_empty());

    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the empty-query hint renders");
}

/// Results render/clear correctly across every query transition
/// (empty hint -> hits -> no-results -> back to the empty hint), each
/// swapping `results_container`'s internal content while the outer Column
/// stays a fixed two children.
#[test]
fn results_render_and_clear_across_query_transitions() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut tcx) = boot_to_search();
    let controller = SearchController::instance();

    // Empty query: the hint view.
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the empty-query hint renders");
    assert!(controller.results.get_untracked().is_empty());

    // Non-empty, matching query: hit rows render. (Driven directly through
    // the controller signal, not a real keystroke: the point of this test is
    // `results_container`'s content across transitions, not per-keystroke
    // field focus, which `typing_a_second_character_keeps_the_field_focused`
    // above already covers.)
    controller.query.set("leadership".to_string());
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    let results = controller.results.get_untracked();
    assert_eq!(
        results.channels.len(),
        1,
        "the \"leadership\" channel matches"
    );
    assert!(scene.glyph_runs > 0, "the matched channel row renders");

    // Non-empty, non-matching query: the no-results view replaces the rows.
    controller.query.set("zzznotarealquery".to_string());
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(
        controller.results.get_untracked().is_empty(),
        "a non-matching query yields no results"
    );
    assert!(scene.glyph_runs > 0, "the no-results message renders");

    // Clearing the query returns to the hint view.
    controller.query.set(String::new());
    let scene = rebuild(&mut root, &mut state, &mut tcx);
    assert!(controller.results.get_untracked().is_empty());
    assert!(scene.glyph_runs > 0, "the hint view renders again");
}
