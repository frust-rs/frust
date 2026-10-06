//! S1 — Animation storm (the bubblebench workload).
//!
//! A 1:1 embedded copy of the former `examples/bubblebench` example's S1
//! workload: physics-driven gradient bubbles (seed 42) with shaped text runs,
//! repainted every frame, plus a Pause/Play + Reset + FPS HUD. The physics
//! ([`physics`]) and canvas widget ([`chart`]) were **copied** into this
//! package rather than path-included from `examples/bubblebench` (the
//! "embed or path-include" choice — embed): bubblebench was a standalone
//! crate excluded from the root workspace, and a cross-package `#[path]`
//! include of its source would have coupled this benchmark to that example's
//! on-disk layout. That embed choice paid off: bubblebench itself was
//! removed (2026-07-25, redundant with — and, unlike — this scenario, which
//! renders correctly) once this copy proved self-contained, with no
//! path-dependent consumer left behind. The shared physics constants
//! and per-frame paint workload stay byte-identical to the original repro
//! (see `physics.rs`'s own module doc for the flutter/flutter#180958 provenance),
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
    AnyView, Color, EdgeInsets, Get, Padding, SizedBox, any, button, column, row, stack, text,
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

        let top_row = row()
            .child(text(fps_label).size(18.0).color(fps_color))
            .flex(1, SizedBox(None, None))
            .child(button(
                if state.running { "Pause" } else { "Play" },
                |state: &mut BenchState| state.running = !state.running,
            ))
            .child(SizedBox(Some(8.0), None))
            .child(button("Reset", |state: &mut BenchState| state.epoch += 1));

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
            column()
                .child(top_row)
                .flex(1, SizedBox(None, None))
                .child(info),
        );

        any(stack()
            .child(bubble_chart(state.running, state.epoch, state.fps))
            .child(hud))
    }
}
