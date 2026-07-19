//! Bubblebench — a Frust port of the Flutter bubble-physics performance
//! repro from flutter/flutter#180958 (Impeller: 5–20 FPS on Adreno 840 where
//! Skia holds ~120).
//!
//! The workload matches the Dart app 1:1: 60 physics-driven bubbles (seed 42),
//! each painted every frame as a radial-gradient circle fill + a stroked
//! border + two shaped text runs (ticker symbol and `+x.x%`), with
//! touch-drag repulsion, an FPS readout, Pause/Play, and Reset. See
//! [`physics`] for the simulation port and [`chart`] for the canvas widget;
//! this module is only the shell — app state, the HUD overlay, and the
//! [`frust::app!`] entry binding all three platforms.
//!
//! Run it with `cargo run` (desktop preview) or `frust run` (Android/iOS).
//! For per-pass frame timings use `FRUST_TRACE=1` (see
//! `docs/DEVELOPMENT.md`'s Instrumentation).

pub mod chart;
pub mod physics;

use frust::{
    AnyView, Axis, Brightness, Color, Component, EdgeInsets, FlexView, Get, Padding, RwSignal,
    SizedBox, Stack, Theme, any, button, flexible, inflexible, set_app_theme, text,
};

use chart::bubble_chart;

/// The FPS readout's traffic-light thresholds, verbatim from the repro.
const FPS_RED_BELOW: f64 = 30.0;
const FPS_ORANGE_BELOW: f64 = 55.0;

/// Application state: the two benchmark controls (plain fields, mutated by the
/// HUD buttons) plus the FPS signal the chart widget writes from its paint
/// pass — paint has no app-state access, so the measurement crosses back on a
/// signal, which is also what re-renders the readout once a second.
pub struct AppState {
    /// Whether the simulation is stepping (Pause/Play).
    pub running: bool,
    /// Bumped by Reset; the chart reseeds when it changes.
    pub epoch: u64,
    /// Measured painted-frames-per-second, written by the chart ~1×/second.
    pub fps: RwSignal<f64>,
}

/// The benchmark's view: the chart canvas filling the window, under a HUD
/// overlay (FPS readout + Pause/Reset top row, an info caption at the bottom).
fn app_logic(state: &mut AppState) -> AnyView<AppState> {
    let fps = state.fps.get();
    let fps_label = if fps > 0.0 {
        format!("FPS: {fps:.1}")
    } else {
        "FPS: —".to_string()
    };
    let fps_color = if fps > 0.0 && fps < FPS_RED_BELOW {
        Color::from_rgb8(0xEF, 0x53, 0x50)
    } else if fps > 0.0 && fps < FPS_ORANGE_BELOW {
        Color::from_rgb8(0xFB, 0x8C, 0x00)
    } else {
        Color::from_rgb8(0x66, 0xBB, 0x6A)
    };

    let top_row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(text(fps_label).size(18.0).color(fps_color)),
            flexible(1, SizedBox(None, None)),
            inflexible(button(
                if state.running { "Pause" } else { "Play" },
                |state: &mut AppState| state.running = !state.running,
            )),
            inflexible(SizedBox(Some(8.0), None)),
            inflexible(button("Reset", |state: &mut AppState| state.epoch += 1)),
        ],
    );

    let info = text(
        "Bubble Physics Performance Test — 60 bubbles with radial-gradient \
         fills and shaped text runs, repainted every frame. Touch to interact. \
         Frust port of the flutter/flutter#180958 Impeller repro.",
    )
    .size(12.0)
    .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xB3));

    let hud = Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(top_row),
                flexible(1, SizedBox(None, None)),
                inflexible(info),
            ],
        ),
    );

    any(Stack(vec![
        any(bubble_chart(state.running, state.epoch, state.fps)),
        any(hud),
    ]))
}

/// The root [`Component`] (spec §5.5): wires [`AppState`] and forces the dark
/// M3 theme once, matching the repro's `ThemeData.dark()` over its
/// `0xFF0D1421` canvas.
#[derive(Default)]
pub struct BubblebenchApp;

impl Component for BubblebenchApp {
    type State = AppState;

    fn init(&self) -> AppState {
        let mut theme = Theme::m3_baseline();
        theme.brightness = Brightness::Dark;
        set_app_theme(theme);
        AppState {
            running: true,
            epoch: 0,
            fps: RwSignal::new(0.0),
        }
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        app_logic(state)
    }
}

// The generated app's sole entry point (spec §5.5/§10): one line binds
// `BubblebenchApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (unconditional;
// self-gated to `target_os = "ios"`), and (on desktop) the hidden
// `__frust_main` that `main.rs` calls.
frust::app!(BubblebenchApp);
