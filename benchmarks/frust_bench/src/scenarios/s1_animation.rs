//! S1 — Animation storm (the bubblebench workload).
//!
//! A 1:1 embedded copy of `examples/bubblebench`'s S1 workload:
//! physics-driven gradient bubbles (seed 42) with shaped text runs, repainted
//! every frame, plus a Pause/Play + Reset + FPS HUD. The physics
//! ([`physics`]) and canvas widget ([`chart`]) are **copied** into this
//! package rather than path-included from `examples/bubblebench` (PLAN 9.E's
//! "embed or path-include" choice — embed): bubblebench is a standalone crate
//! excluded from the root workspace, and a cross-package `#[path]` include of
//! its source would couple this benchmark to that example's on-disk layout.
//! The copy keeps `frust_bench` self-contained; the shared physics constants
//! and per-frame paint workload stay byte-identical to the repro (see
//! `physics.rs`'s own module doc for the flutter/flutter#180958 provenance),
//! **except** bubble geometry: radius and initial cluster spread are
//! fractions of the SafeArea-inset play area's `min(width, height)`, and
//! bubble **count** is derived from the play area (not fixed) so the field
//! can settle instead of staying jam-packed — per the canonical S1 spec
//! (contract home: `benchmarks/flutter_bench/lib/bench/datasets.dart`; see
//! `physics.rs`), so both the bubble-to-play-area ratio and the derived
//! count match the Flutter side exactly. The scenario host is wrapped in the
//! facade's `SafeArea` in `crate::BenchApp::build`.
//!
//! Scenario markers: the driver's default [`Scenario::on_start`]/
//! [`on_end`](Scenario::on_end) bracket the whole animation window as `s1`;
//! since a live 60-bubble field never settles (a sustained limit cycle),
//! the raw per-frame stream inside that window is the animation-storm series
//! the harness slices.

pub mod chart;
pub mod physics;

use frust::{
    AnyView, Axis, Color, EdgeInsets, FlexView, Get, Padding, SizedBox, Stack, any, button,
    flexible, inflexible, text,
};

use super::{BenchState, Scenario};
use chart::bubble_chart;

/// The FPS readout's traffic-light thresholds, verbatim from the repro.
const FPS_RED_BELOW: f64 = 30.0;
const FPS_ORANGE_BELOW: f64 = 55.0;

/// S1 — Animation storm.
pub struct S1;

impl Scenario for S1 {
    fn id(&self) -> &'static str {
        "s1"
    }

    fn title(&self) -> &'static str {
        "Animation storm (bubble count derived per play area, perpetual physics)"
    }

    fn build(&self, state: &mut BenchState) -> AnyView<BenchState> {
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
                    |state: &mut BenchState| state.running = !state.running,
                )),
                inflexible(SizedBox(Some(8.0), None)),
                inflexible(button("Reset", |state: &mut BenchState| state.epoch += 1)),
            ],
        );

        let info = text(
            "S1 Animation storm — physics-driven bubbles (count derived per \
             play area) with radial-gradient fills and shaped text runs, \
             repainted every frame. Touch to interact. Frust port of the \
             flutter/flutter#180958 Impeller repro.",
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
}
