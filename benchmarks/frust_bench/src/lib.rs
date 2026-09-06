//! frust_bench — the Frust side of the cross-framework benchmark
//! suite. A single app hosting every scenario behind one
//! [`scenarios::Scenario`] driver contract, selected via a deep link
//! (`frustbench://<id>`) or the `FRUST_BENCH_SCENARIO` env var and switchable at
//! runtime from a HUD button row.
//!
//! The driver, the registry, and the eight frame-class scenarios live here:
//! S1 (animation storm, ported from the since-removed `examples/bubblebench`;
//! see `scenarios::s1_animation`'s module doc for the provenance) through S8
//! (plugin-call overhead), plus the two `d*` DB op-latency scenarios
//! (`benchmarks/PROTOCOL.md` §9).
//!
//! # Cargo features
//!
//! * `db` (**default**) — the `d1`/`d2` DB op-latency scenarios and their
//!   `frust-database` dependency. On by default so every existing invocation
//!   (`frust run`, `frust build`, `cargo test`, the harness) is unchanged;
//!   `--no-default-features` builds the S1–S8 frame-class app alone, without
//!   the bundled SQLite, which is the arm the app-size matrix measures. See
//!   `Cargo.toml`'s feature comment for the exact build invocation.
//! * `lean` — the release-only `log` ceiling the CLI passes for `--release`.
//!
//! Measure a run with `FRUST_TRACE=1 FRUST_TRACE_RAW=1` — the shell emits one
//! parseable `frust-perf raw ...` line per frame plus the driver's
//! `bench-scenario-start/end <id>` markers, which the harness slices
//! by (see `docs/DEVELOPMENT.md`'s Instrumentation).
//!
//! Run it with `cargo run` (desktop preview, S1 by default) or
//! `FRUST_BENCH_SCENARIO=s7 cargo run` to pick a scenario; `frust run` drives
//! Android/iOS (deep-link selection there).

pub mod scenarios;

use frust::{
    AnyView, Axis, Brightness, Component, EdgeInsets, FlexChild, FlexView, Get, Padding, Set,
    SizedBox, Stack, any, button, flexible, inflexible, safe_area, set_app_theme,
};

use scenarios::{BenchState, SCENARIOS};

/// The root [`Component`]: owns the shared [`BenchState`], forces
/// the dark M3 theme (matching S1's `0xFF0D1421` canvas), and drives the
/// scenario switch — reconciling the HUD/deep-link `selected` request against
/// the mounted `active` scenario each rebuild and firing the
/// [`scenarios::Scenario`] start/end markers on a change.
#[derive(Default)]
pub struct BenchApp;

impl Component for BenchApp {
    type State = BenchState;

    fn init(&self) -> BenchState {
        let mut theme = frust_material::baseline();
        theme.brightness = Brightness::Dark;
        set_app_theme(theme);
        let state = BenchState::new();
        // Open the launch scenario's marker window (build reconciles later
        // switches, but never fires for the initial no-op `want == active`).
        SCENARIOS[state.active].on_start();
        state
    }

    fn build(&self, state: &mut BenchState) -> AnyView<BenchState> {
        // Fold a warm deep link into `selected` before reconciling it.
        state.consume_deep_link();

        // Reconcile the requested scenario against the mounted one, bracketing
        // the switch with end/start markers.
        let want = state.selected.get().min(SCENARIOS.len() - 1);
        if want != state.active {
            SCENARIOS[state.active].on_end();
            SCENARIOS[want].on_start();
            state.active = want;
        }

        let scenario_view = SCENARIOS[state.active].build(state);

        // Host the scenario (and the dev switcher) inside `SafeArea` so content
        // renders within the system-bar/cutout insets rather than edge-to-edge
        // behind the status/nav bars — mirroring the Flutter app's
        // `Scaffold > SafeArea` host (`flutter_bench/lib/main.dart`). This is
        // also the S1 size-parity contract's play area: `min(playW, playH)` is
        // measured over the SafeArea-inset region on both apps (see
        // `benchmarks/flutter_bench/lib/bench/datasets.dart`'s S1 spec).
        any(safe_area(Stack(vec![
            scenario_view,
            any(scenario_switcher(state)),
        ])))
    }
}

/// A bottom-anchored row of one button per scenario (a desktop dev convenience;
/// the harness selects via deep link / env). The active scenario's button is
/// prefixed with `*`. Clicking sets [`BenchState::selected`], which wakes the
/// rebuild that switches scenarios.
fn scenario_switcher(state: &BenchState) -> AnyView<BenchState> {
    let active = state.active;
    let mut buttons: Vec<FlexChild<BenchState>> = Vec::with_capacity(SCENARIOS.len() * 2);
    for (i, scenario) in SCENARIOS.iter().enumerate() {
        let label = if i == active {
            format!("*{}", scenario.id())
        } else {
            scenario.id().to_string()
        };
        buttons.push(inflexible(button(label, move |s: &mut BenchState| {
            s.selected.set(i);
        })));
        buttons.push(inflexible(SizedBox(Some(6.0), None)));
    }
    let row = FlexView::new(Axis::Horizontal, buttons);

    // Push the row to the bottom edge with a flexible top spacer.
    any(FlexView::new(
        Axis::Vertical,
        vec![
            flexible(1, SizedBox(None, None)),
            inflexible(Padding(EdgeInsets::all(8.0), row)),
        ],
    ))
}

// The generated app's sole entry point: one line binds
// `BenchApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (self-gated to
// `target_os = "ios"`), and (on desktop) the hidden `__frust_main` that
// `main.rs` calls.
frust::app!(BenchApp);
