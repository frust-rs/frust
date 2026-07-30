//! layer-bench — a composite-cost spike bench for the retained-layers
//! feature.
//!
//! ============================ THROWAWAY LAB ============================
//! This is a **measurement lab, not shipped API surface.** It drives vello's
//! composite primitives directly through `frust::authoring` — the facade's
//! widget-authoring seam (`docs/CODE_STANDARDS.md`'s State & Reactivity
//! Conventions) — so the spike can answer the one
//! open question with device numbers: does compositing pre-rendered
//! textures beat re-rasterizing the equivalent vector content in vello 0.9?
//! Nothing here is meant to become framework code — the real feature (if the
//! spike says GO) builds a proper `LayerCache` in `frust-render`.
//! ======================================================================
//!
//! # The three modes (tap anywhere to cycle; A -> B -> B+1relayer -> A)
//!
//! - **Scene A ([`Mode::Vector`])** — a Feedback-screen-equivalent vector
//!   scene (badges/tags/alerts/toasts/loaders, calibrated to comparable
//!   path/glyph counts as `examples/glyph-catalog/src/pages/feedback.rs`),
//!   re-rasterized every frame. The full-vector baseline.
//! - **Scene B ([`Mode::Composite`])** — the same screen area pre-rendered into
//!   [`scene::NUM_LAYERS`] offscreen `Rgba8Unorm` image quads (dedicated
//!   textures, composited via `draw_image` — the same vello path the real
//!   feature's texture-override composite lowers to), re-rendered **zero**
//!   times per frame, plus one small live vector region (an orbiting spinner)
//!   so the scene isn't trivially static.
//! - **Scene B+1relayer ([`Mode::CompositeRelayer`])** — scene B, but one layer
//!   is a `ShaderQuad` re-rendered into its own dedicated target **every frame**
//!   via `frust-render`'s `shader_effects` pre-pass — the `render_to_texture`-
//!   style per-layer dispatch overhead the spike isolates (B+1relayer minus B
//!   = the cost of one dirty-boundary re-capture per frame).
//!
//! # Driving it
//!
//! Tapping the screen cycles the mode; the current mode shows as a top-left
//! HUD label so a device operator can confirm it visually. Because env vars
//! don't propagate to an Android process at runtime, the **initial** mode is
//! read from the compile-time `LAYER_BENCH_SCENE` define (`a`|`b`|`c`), parsed
//! exactly like the framework's own `FRUST_*` switches (`option_env!` +
//! runtime fallback):
//!
//! ```text
//! # desktop preview, mode B:
//! LAYER_BENCH_SCENE=b cargo run
//! # profiled APK, mode C, with the perf trace on:
//! frust build apk --profile --define LAYER_BENCH_SCENE=c --define FRUST_TRACE=1 --define FRUST_TRACE_RAW=1
//! ```
//!
//! `LAYER_BENCH_DPR` (default `3.0`) sets the device-pixel ratio the offscreen
//! textures are sized against, so their memory/bandwidth reflect physical
//! resolution — set it to the target device's DPR for a fair composite-fill
//! comparison (e.g. `--define LAYER_BENCH_DPR=2.75` for the Xiaomi 12).
//!
//! Every mode requests continuous frames, so `FRUST_TRACE=1 FRUST_TRACE_RAW=1`
//! (compiled in for debug/profile builds) emit one `frust-perf raw ...` line
//! per frame at the refresh rate — the steady-state per-frame series the spike
//! compares (submit_us / encode_us).

pub mod bench_view;
pub mod scene;

use frust::{
    AnyView, Axis, Brightness, Color, Component, EdgeInsets, FlexView, GestureDetector, Get,
    GetUntracked, Padding, RwSignal, Set, SizedBox, Stack, Theme, any, flexible, inflexible,
    safe_area, set_app_theme, text,
};

use bench_view::bench_view;
use scene::Mode;

/// Default device-pixel ratio for sizing the offscreen textures when
/// `LAYER_BENCH_DPR` is unset. ~3.0 brackets modern flagships (Xiaomi 12 ≈
/// 2.75, OnePlus 15 similar); set the define to the exact device DPR for a
/// bandwidth-faithful composite comparison.
const DEFAULT_DPR: f64 = 3.0;

/// The bench's reactive state: the current mode (cycled by tap) and the
/// texture DPR (fixed at mount from the compile-time/runtime define).
pub struct AppState {
    mode: RwSignal<Mode>,
    dpr: f64,
}

/// The initial [`Mode`], from the compile-time `LAYER_BENCH_SCENE` define
/// (Android-safe) or the runtime env var (desktop), defaulting to
/// [`Mode::Vector`]. Compile-time wins so an APK built with `--define
/// LAYER_BENCH_SCENE=b` launches in scene B without any runtime env.
pub fn initial_mode() -> Mode {
    option_env!("LAYER_BENCH_SCENE")
        .map(str::to_string)
        .or_else(|| std::env::var("LAYER_BENCH_SCENE").ok())
        .map(|raw| Mode::parse(&raw))
        .unwrap_or(Mode::Vector)
}

/// The configured texture DPR, from `LAYER_BENCH_DPR` (compile-time then
/// runtime), defaulting to [`DEFAULT_DPR`]. A non-positive/garbage value falls
/// back to the default.
pub fn configured_dpr() -> f64 {
    option_env!("LAYER_BENCH_DPR")
        .map(str::to_string)
        .or_else(|| std::env::var("LAYER_BENCH_DPR").ok())
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(DEFAULT_DPR)
}

/// The top-left HUD: the current mode label so an operator can confirm the
/// active scene visually. Pinned to the top via a vertical flex with a
/// trailing spacer, inside the safe area.
fn hud(mode: Mode, dpr: f64) -> AnyView<AppState> {
    let label = text(format!("layer-bench · {}  ·  dpr {dpr:.2}", mode.label()))
        .size(14.0)
        .color(Color::from_rgb8(0xF2, 0xF4, 0xF8));
    let hint = text("tap anywhere to cycle scene")
        .size(11.0)
        .color(Color::from_rgba8(0xF2, 0xF4, 0xF8, 0xB0));

    any(safe_area(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(label),
                inflexible(SizedBox(None, Some(4.0))),
                inflexible(hint),
                flexible(1, SizedBox(None, None)),
            ],
        ),
    )))
}

/// The whole screen: the bench canvas under the HUD, all wrapped in a
/// [`GestureDetector`] so a tap anywhere cycles the mode. The canvas and HUD
/// don't consume taps, so the wrapping detector receives every tap.
fn app_logic(state: &mut AppState) -> AnyView<AppState> {
    let mode = state.mode.get();
    let dpr = state.dpr;

    let stack = Stack(vec![any(bench_view(mode, dpr)), hud(mode, dpr)]);

    any(GestureDetector(stack).on_tap(|state: &mut AppState| {
        let next = state.mode.get_untracked().next();
        state.mode.set(next);
    }))
}

/// The root [`Component`]: forces a dark theme and seeds the
/// reactive state from the compile-time/runtime configuration.
#[derive(Default)]
pub struct LayerBenchApp;

impl Component for LayerBenchApp {
    type State = AppState;

    fn init(&self) -> AppState {
        let mut theme = Theme::m3_baseline();
        theme.brightness = Brightness::Dark;
        set_app_theme(theme);
        AppState {
            mode: RwSignal::new(initial_mode()),
            dpr: configured_dpr(),
        }
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        app_logic(state)
    }
}

// The generated app's sole entry point: one line binds
// `LayerBenchApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (self-gated to
// `target_os = "ios"`), and (on desktop) the hidden `__frust_main` that
// `main.rs` calls.
frust::app!(LayerBenchApp);
