//! Ported regression coverage from the three examples this app's showcase
//! absorbs, adapted onto the [`ShellApp`]/[`ShellState`] shape (see
//! `docs/DEVELOPMENT.md`'s deleted-example test-migration note):
//!
//! - `examples/catalog`'s `catalog_builds_switches_tabs_and_completes_a_modal_round_trip`
//!   → [`catalog_builds_switches_tabs_and_completes_a_modal_round_trip`] below:
//!   the widgets tabs became the showcase's `/widgets/*` routes, and the
//!   catalog's own app-state `last_modal_result` signal became the shell-wide
//!   [`ShellSignals::last_result`] (the same catalog toolbar-toggle +
//!   `push_transparent_for_result` round trip, just landing in the shell's own
//!   banner signal instead of a catalog-local one).
//! - `examples/navdemo`'s `route_table_builds_and_state_survives_push_pop` →
//!   [`route_table_builds_and_state_survives_push_pop`] below: same
//!   push/pop/deep-link shape, adapted to the showcase's route table and its
//!   `ShellSignals::transition` field (the navdemo original's `transition_choice`
//!   was a bare `AppState` field the *navigator's own pages* could see get torn
//!   down and rebuilt across the round trip; here it lives in the shell-level
//!   `ShellSignals` bundle, entirely outside any page's own retained state, so
//!   this port's "survives" is a structurally weaker but still real proof: a
//!   pushed/popped page never zeroes it).
//! - `examples/gallery`'s `build_succeeds_for_both_brightness_states` →
//!   [`shell_builds_under_both_brightness_states`] below: this app has no
//!   `GalleryState`-style per-app dark toggle field — brightness is instead
//!   ambient `Theme` state (task 06's settings screen), so the port drives it
//!   the same way [`matrix_smoke`]'s route matrix does: `provide_context` a
//!   `Theme` at each brightness before rebuilding.
//!
//! Each ported test preserves the *assertions* of its original (root builds
//! without panic; a real, mounted `NavigatorController`'s push/pop/deep-link op
//! queue drains through rebuild; app state is unaffected by a page's own
//! teardown) even where the exact state shape it reads had to change.
//!
//! Deliberately one `ReactiveRuntime::init` + `push_deep_link` test per binary
//! consideration (mirrored from `examples/navdemo`'s own test doc): only
//! [`route_table_builds_and_state_survives_push_pop`] below calls
//! `push_deep_link` (a process-wide signal) in this file, so it can't race a
//! sibling test in the same binary that also touches it.

use forgekit::{AnyView, Component, GetUntracked, PopResult, Set, Theme, TransitionSpec};
use forgekit_core::{InputEvent, PointerButton, PointerEvent, PointerPhase, RenderRoot};
use kurbo::Point;

use team_demo::{ShellApp, ShellState};

mod support;
use support::setup;

type Root = RenderRoot<ShellState, AnyView<ShellState>>;

/// Ported from `examples/catalog` (see the [module docs](self)).
#[test]
fn catalog_builds_switches_tabs_and_completes_a_modal_round_trip() {
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = ShellApp.init();
    let mut logic = |s: &mut ShellState| ShellApp.build(s);

    // Criterion: the initial scaffold (the `/` roster tab) builds with no
    // panic.
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "the root shell must build at \"/\""
    );

    // Criterion: switching through every widgets exhibit rebuilds without
    // panicking — the catalog original's per-tab loop, ported onto this app's
    // `/widgets/*` route family (`go` resets the stack to each route in turn,
    // the navdemo/router precedent for a location-by-location smoke walk).
    for route in [
        "/widgets/controls",
        "/widgets/cards",
        "/widgets/modals",
        "/widgets/appbars",
    ] {
        state.nav.router().go(route);
        root.rebuild(&mut logic, &mut state);
        assert!(
            root.root_id().is_some(),
            "the shell must rebuild on route {route:?}"
        );
    }

    // Criterion: a live Material -> Cupertino theme swap (the settings
    // screen's real effect, simulated here the same way the shell's own
    // `use_context::<Theme>()` read is fed — see `docs/ARCHITECTURE.md`'s
    // Theme delivery) rebuilds without panicking.
    forgekit::provide_context(Theme::cupertino_baseline());
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "the shell must rebuild under the Cupertino baseline"
    );

    // Criterion (the state-dependent assertion, not a pure helper): a modal
    // pushed transparently through the *real*, mounted `NavigatorController`
    // and popped with a result must deliver that `PopResult` back into app
    // state via `on_result` — this only holds if the navigator's
    // op-queue-drain/rebuild pipeline actually ran, not merely that a signal
    // was set and read back.
    state.nav.router().go("/");
    root.rebuild(&mut logic, &mut state);
    let last_result = state.signals.last_result;
    let controller = state.nav.router().controller().clone();
    controller.push_transparent_for_result(
        || forgekit::any(forgekit::text("probe dialog")),
        TransitionSpec::NONE,
        move |_s: &mut ShellState, result: PopResult| {
            last_result.set(result.take::<String>());
        },
    );
    root.rebuild(&mut logic, &mut state); // the pushed page's first build
    controller.pop_with_result(PopResult::of("confirmed".to_string()));
    root.rebuild(&mut logic, &mut state); // the structural pop applies here
    assert_eq!(
        last_result.get_untracked(),
        None,
        "on_result is queued, not yet flushed (no event pass)"
    );
    // The next event pass flushes the queued on_result callback with
    // `&mut State` (mirrors `nav::navigator`'s own
    // `pop_result_reaches_callback_with_state` test).
    root.event(
        &mut state,
        &InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Primary,
        }),
    );
    assert_eq!(
        last_result.get_untracked(),
        Some("confirmed".to_string()),
        "a modal push/pop round trip through the real NavigatorController must \
         deliver its PopResult back into app state"
    );
}

/// Ported from `examples/navdemo` (see the [module docs](self)).
#[test]
fn route_table_builds_and_state_survives_push_pop() {
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = ShellApp.init();
    let mut logic = |s: &mut ShellState| ShellApp.build(s);

    // Criterion: the root builds at "/", the navigator's resolved start
    // location (rebuild only — no layout/paint, so no `TextContext` is
    // needed; see `docs/ARCHITECTURE.md`'s Frame pipeline).
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "the root shell must build at \"/\""
    );

    // Criterion: a matcher-driven push resolves a second location.
    state.nav.router().push("/member/3");
    root.rebuild(&mut logic, &mut state);
    assert!(root.root_id().is_some());

    // Criterion: a third, distinct matcher-driven location.
    state.nav.router().push("/settings");
    root.rebuild(&mut logic, &mut state);
    assert!(root.root_id().is_some());

    // Criterion: a warm deep link (the desktop dev seam, the same one the
    // nav playground's "simulate deep link" button drives) reaches a member
    // id no visible list button leads to.
    forgekit::push_deep_link("/member/99");
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "a deep link to an id-parameterized route must resolve and build"
    );

    // Criterion: retained shell state survives a push/pop round trip — the
    // nav playground's transition-preset choice must still read back
    // afterward. `TransitionChoice` is a private type (`mod shell` — see the
    // crate's placeholder-contract docs), so this port reads the value rather
    // than naming/constructing a specific variant: it snapshots whatever the
    // live default is, then proves the round trip below leaves it untouched
    // (the original navdemo test instead set a specific `Ios` variant before
    // the round trip; see the module docs' note on this structural
    // difference).
    let before = state.signals.transition.get_untracked();
    state.nav.router().push("/member/1");
    root.rebuild(&mut logic, &mut state);
    state.nav.router().controller().pop();
    root.rebuild(&mut logic, &mut state);
    assert_eq!(
        state.signals.transition.get_untracked(),
        before,
        "shell-level state must survive a navigator push/pop round trip"
    );
}

/// Ported from `examples/gallery` (see the [module docs](self)).
#[test]
fn shell_builds_under_both_brightness_states() {
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = ShellApp.init();
    let mut logic = |s: &mut ShellState| ShellApp.build(s);

    forgekit::provide_context(Theme::m3_baseline());
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "the shell must build under the light baseline"
    );

    forgekit::provide_context(Theme::m3_baseline().with_brightness(forgekit::Brightness::Dark));
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "the shell must build under the dark baseline"
    );
}
