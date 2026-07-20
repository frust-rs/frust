//! S7 — Cold start + idle.
//!
//! Two halves of the same "thin runtime" story (PLAN 9.E's S7):
//!
//! 1. **Cold start.** No app-side code is needed here — every shell already
//!    records the [`StartupSpans`](frust_shell_common::perf::StartupSpans)
//!    milestones (`native_lib_load` → `init_entry` → `adapter_ready` →
//!    `device_ready` → `renderer_ready` → `first_rebuild_done` →
//!    `first_frame_presented`) and emits one `frust-perf startup ...` line on
//!    the first presented frame when `FRUST_TRACE` is set (see
//!    `docs/ARCHITECTURE.md`'s `FrameStats`/`StartupSpans` row). The harness
//!    reads that line plus the platform's own `am start -W` / launch timing.
//!    Selecting `frustbench://s7` launches straight into this static screen so
//!    the startup path is measured with a trivial first frame.
//!
//! 2. **Idle.** After the first frame this scenario is deliberately *static*:
//!    it requests no further frames, so the mobile frame gate (`Skip`) and the
//!    desktop `ControlFlow::Wait` loop both go fully idle — the property S7
//!    measures against Flutter's continuous vsync. The [`IdleSentinel`] widget
//!    stamps a `s7-idle` scenario marker on the first frame to open the idle
//!    window; the authoritative window bound is that marker's timestamp plus
//!    [`IDLE_WINDOW_SECS`] (the harness slices idle CPU/memory over it). A
//!    best-effort closing marker fires if any later frame happens to land past
//!    the window (an input, a resize) — it cannot self-schedule one, by design,
//!    since scheduling a wake-up would defeat the idle measurement.

use frust::{
    AnyView, Axis, Color, EdgeInsets, FlexView, FrameTime, Padding, any, inflexible, text,
};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::Size;

use super::{BenchState, Scenario};

/// The idle window the harness measures idle CPU + sustained memory over,
/// opened by [`IdleSentinel`]'s first-frame marker.
pub const IDLE_WINDOW_SECS: f64 = 60.0;

/// The idle-window scenario-marker name (a sub-marker under `s7`).
const IDLE_MARKER: &str = "s7-idle";

/// S7 — Cold start + idle.
pub struct S7;

impl Scenario for S7 {
    fn id(&self) -> &'static str {
        "s7"
    }

    fn title(&self) -> &'static str {
        "Cold start + idle (thin runtime, frame-gate idle)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        let body = Padding(
            EdgeInsets::all(24.0),
            FlexView::new(
                Axis::Vertical,
                vec![
                    inflexible(text("S7 — Cold start + idle").size(24.0)),
                    inflexible(
                        text(
                            "Launch-to-first-frame is measured from the shell's own \
                             StartupSpans (FRUST_TRACE=1 emits a `frust-perf startup` line \
                             on the first presented frame) plus the platform launch timer. \
                             After this frame the app requests no more frames — the frame \
                             gate goes idle for the 60s idle-CPU/memory window.",
                        )
                        .size(14.0)
                        .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xB3)),
                    ),
                ],
            ),
        );

        // The sentinel opens the idle-window marker on the first frame and then
        // stays silent (requests no frames), so the whole screen goes idle.
        any(FlexView::new(
            Axis::Vertical,
            vec![inflexible(any(idle_sentinel())), inflexible(any(body))],
        ))
    }
}

/// A zero-size, paint-free widget whose only job is to stamp the idle-window
/// scenario markers off the shell frame clock. See the module docs.
pub struct IdleSentinel;

/// Construct an [`IdleSentinel`] view.
pub fn idle_sentinel() -> IdleSentinel {
    IdleSentinel
}

/// The retained sentinel: the first-frame clock reading and whether the closing
/// marker has already fired.
pub struct IdleSentinelWidget {
    /// Frame clock reading of the first painted frame (idle-window open).
    started_at: Option<FrameTime>,
    /// Whether the best-effort closing marker has been stamped.
    closed: bool,
}

impl<State: 'static> View<State> for IdleSentinel {
    type Element = IdleSentinelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> IdleSentinelWidget {
        IdleSentinelWidget {
            started_at: None,
            closed: false,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut IdleSentinelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for IdleSentinelWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
        Size::ZERO
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        match self.started_at {
            None => {
                // First frame: open the idle window and go idle (request nothing).
                self.started_at = Some(now);
                frust_shell_common::perf::mark_scenario_start(IDLE_MARKER);
            }
            Some(start) => {
                // Best-effort close if a later frame lands past the window; the
                // sentinel never schedules this frame itself (that would defeat
                // the idle measurement — see the module docs).
                if !self.closed && now.saturating_sub(start).as_secs_f64() >= IDLE_WINDOW_SECS {
                    self.closed = true;
                    frust_shell_common::perf::mark_scenario_end(IDLE_MARKER);
                }
            }
        }
    }
}
