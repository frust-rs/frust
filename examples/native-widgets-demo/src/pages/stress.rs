//! **Stress** — the gate harness: a measurement rig, not a design pattern.
//!
//! - **Live slot readout** — `live_slot_count` (a diagnostics accessor with
//!   no compatibility promise), read against every page's documented at-rest
//!   count, tabulated here from [`AT_REST_SLOTS`].
//! - **Mount/unmount cycler** — its OWN six-control group (separate from the
//!   Controls page), mounted and unmounted every [`CYCLE_STEP_MS`]: each
//!   mount allocates six fresh native slots, each unmount tears them down,
//!   exercising the teardown-retire path. The live count must dip and climb
//!   by 6 promptly rather than lingering for the differ's idle backstop.
//! - **50-slot stress toggle** — [`STRESS_SLOT_COUNT`] single-control
//!   `native_label` slots (the cheapest control), off by default; scroll the
//!   page while it is on to observe scroll-batch jank and differ batch sizes.
//!
//! The cycler's state is a pure function of wall-clock time since the run
//! started ([`cycle_state`]); while a run has cycles left, [`GateFrameTicker`]
//! requests a frame every paint so the page keeps re-evaluating it. No
//! nested scroll view: the shell's own scroll view already covers the page.
//!
//! Zero native slots at rest; +6 while the cycler's group is mounted; +50
//! with the stress toggle on.

use std::time::Instant;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::{AnyView, Axis, FlexChild, FlexView, Get, GetUntracked, Set, any, column, inflexible};
use frust_native_widgets::{
    NativeImageFit, native_button, native_image, native_label, native_progress, native_slider,
    native_switch,
};

use super::common::{
    AT_REST_SLOTS, S, block, caption, chip, demo_image_bytes, gap, gap_h, label, local_sig,
    page_column, page_header, readout, row,
};
use crate::SECTION_LABELS;

/// This page's index in [`SECTION_LABELS`].
const SECTION: usize = 7;

/// How many single-control slots the stress toggle mounts.
pub const STRESS_SLOT_COUNT: u32 = 50;

/// The cycler's per-half-step duration (mount, then unmount, is two half-steps
/// = one cycle): slow enough for a person to see the group blink, fast enough
/// that a 100-cycle run takes `100 * 2 * 150 ms` = 30 s. An approximate,
/// human-observable pace — no spec pins it.
pub const CYCLE_STEP_MS: f64 = 150.0;

local_sig!(cycle_target_sig, u32, 0); // 0 == no run started yet
local_sig!(stress_visible_sig, bool, false);

/// A zero-size sentinel whose only job is an unconditional
/// [`PaintCtx::request_frame`] — mounted only while [`cycle_state`] reports a
/// run still has cycles left, so every continuation frame traces to it.
struct GateFrameTicker;

impl<State: 'static> View<State> for GateFrameTicker {
    type Element = GateFrameTickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GateFrameTickerWidget {
        GateFrameTickerWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut GateFrameTickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::PAINT
    }
}

/// The retained widget for a [`GateFrameTicker`].
struct GateFrameTickerWidget;

impl Widget for GateFrameTickerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame();
    }
}

thread_local! {
    /// Wall-clock start of the current run — reset by every
    /// [`start_cycle_run`], so re-tapping restarts from cycle 0.
    static CYCLE_START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Start (or restart) a run of `target` cycles. Writing
/// [`cycle_target_sig`] also wakes an idle page (it reads it tracked).
fn start_cycle_run(target: u32) {
    CYCLE_START.with(|cell| cell.set(Some(Instant::now())));
    cycle_target_sig().set(target);
}

/// Where a `target`-cycle run stands after `elapsed_ms`: `(mounted,
/// completed, running)` — whether the group shows now, how many full
/// mount+unmount pairs finished, and whether cycles remain (so the ticker
/// must stay mounted). `target == 0` is the idle state.
pub fn cycle_phase(target: u32, elapsed_ms: f64) -> (bool, u32, bool) {
    if target == 0 {
        return (false, 0, false);
    }
    let half_steps = (elapsed_ms / CYCLE_STEP_MS).floor() as u64;
    let total_half_steps = u64::from(target) * 2;
    if half_steps >= total_half_steps {
        return (false, target, false);
    }
    let completed = (half_steps / 2) as u32;
    let mounted = half_steps.is_multiple_of(2);
    (mounted, completed, true)
}

/// [`cycle_phase`] against the wall clock since [`start_cycle_run`].
fn cycle_state(target: u32) -> (bool, u32, bool) {
    let elapsed_ms = CYCLE_START.with(|cell| {
        cell.get()
            .map(|start| start.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    });
    cycle_phase(target, elapsed_ms)
}

/// The cycler's own six-control group — separate instances from the Controls
/// page, display-only: the point is create/dispose churn, not interaction.
fn cycle_group(mounted: bool) -> AnyView<S> {
    if !mounted {
        return caption("(cycler group currently unmounted)");
    }
    any(column()
        .child(
            native_button("Cycle button")
                .content_description("Gate-harness cycler button")
                .size(160.0, 40.0),
        )
        .push(gap(4.0))
        .child(
            native_label("Cycle label")
                .content_description("Gate-harness cycler label")
                .size(160.0, 28.0),
        )
        .push(gap(4.0))
        .child(
            native_switch(false)
                .content_description("Gate-harness cycler switch")
                .size(70.0, 32.0),
        )
        .push(gap(4.0))
        .child(
            native_slider(0, 0, 100)
                .content_description("Gate-harness cycler slider")
                .size(200.0, 32.0),
        )
        .push(gap(4.0))
        .child(
            native_progress(0, 0, 100)
                .content_description("Gate-harness cycler progress")
                .size(200.0, 20.0),
        )
        .push(gap(4.0))
        .child(
            native_image(demo_image_bytes())
                .fit(NativeImageFit::Contain)
                .content_description("Gate-harness cycler image")
                .size(48.0, 48.0),
        ))
}

/// [`STRESS_SLOT_COUNT`] `native_label` slots in a plain column.
fn stress_grid() -> AnyView<S> {
    let rows: Vec<FlexChild<S>> = (1..=STRESS_SLOT_COUNT)
        .map(|i| {
            inflexible(
                native_label(format!("Stress slot {i:02}"))
                    .content_description(format!("Gate-harness stress slot {i}"))
                    .size(220.0, 28.0),
            )
        })
        .collect();
    any(FlexView::new(Axis::Vertical, rows))
}

/// The at-rest table: every page's documented count for this build.
fn at_rest_table() -> FlexChild<S> {
    let mut rows = vec![
        inflexible(label(
            "Documented native slots at rest, per page (this build)",
        )),
        gap(4.0),
        inflexible(caption(
            "Only the showing page is mounted, so after a page settles the live count above \
             equals its entry here (Re-read after switching pages).",
        )),
        gap(6.0),
    ];
    for (label_text, count) in SECTION_LABELS.iter().zip(AT_REST_SLOTS) {
        rows.push(inflexible(readout(format!("{label_text}: {count}"))));
    }
    block(rows)
}

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
    // Tracked, so a chip tap (a signal write) wakes the page even at rest.
    let cycle_target = cycle_target_sig().get();
    let stress_visible = stress_visible_sig().get();
    let (cycle_mounted, cycle_completed, cycle_running) = cycle_state(cycle_target);

    let intro = block(vec![
        inflexible(label("GATE HARNESS \u{2014} measurement rig, not a demo")),
        gap(6.0),
        inflexible(caption(
            "Proves the registry's teardown-retire path and gives the scroll-batch stress \
             something to measure. The header's live count samples every rebuild, and \
             rebuilds every frame while a cycler run is going.",
        )),
    ]);

    let cycle_status = if cycle_target == 0 {
        "No cycle run started yet.".to_string()
    } else {
        format!(
            "Cycle {cycle_completed}/{cycle_target} \u{2014} group is currently {}",
            if cycle_mounted {
                "MOUNTED"
            } else {
                "unmounted"
            }
        )
    };
    let cycler = block(vec![
        inflexible(label("Mount/unmount cycler (leak + prompt teardown)")),
        gap(6.0),
        inflexible(caption(
            "Runs its OWN six-control group. Watch the live count dip by 6 on every unmount and \
             climb by 6 on every mount \u{2014} settling immediately, not lingering for a few \
             frames.",
        )),
        gap(6.0),
        inflexible(row(vec![
            inflexible(chip("Cycle x5", |_: &mut S| start_cycle_run(5))),
            gap_h(8.0),
            inflexible(chip("Cycle x100", |_: &mut S| start_cycle_run(100))),
        ])),
        gap(6.0),
        inflexible(readout(cycle_status)),
        gap(6.0),
        inflexible(cycle_group(cycle_mounted)),
    ]);

    let stress_toggle = block(vec![
        inflexible(label(
            "50-slot stress (scroll-batch jank + differ batch sizes)",
        )),
        gap(6.0),
        inflexible(caption(
            "50 single-control (`native_label`) slots, off by default. Scroll the page while \
             it is on to observe batch sizes and jank.",
        )),
        gap(6.0),
        inflexible(chip(
            if stress_visible {
                "Hide 50-slot stress"
            } else {
                "Show 50-slot stress"
            },
            |_: &mut S| {
                let sig = stress_visible_sig();
                sig.set(!sig.get_untracked());
            },
        )),
    ]);

    let mut children = vec![
        page_header(SECTION),
        intro,
        at_rest_table(),
        cycler,
        stress_toggle,
    ];
    if stress_visible {
        children.push(block(vec![inflexible(stress_grid())]));
    }
    if cycle_running {
        children.push(inflexible(GateFrameTicker));
    }
    page_column(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_target_is_settled() {
        assert_eq!(cycle_phase(0, 10_000.0), (false, 0, false));
    }

    #[test]
    fn a_run_alternates_mounted_and_unmounted_then_settles() {
        assert_eq!(cycle_phase(2, 0.0), (true, 0, true));
        assert_eq!(cycle_phase(2, CYCLE_STEP_MS), (false, 0, true));
        assert_eq!(cycle_phase(2, 2.0 * CYCLE_STEP_MS), (true, 1, true));
        assert_eq!(cycle_phase(2, 3.0 * CYCLE_STEP_MS), (false, 1, true));
        assert_eq!(cycle_phase(2, 4.0 * CYCLE_STEP_MS), (false, 2, false));
    }
}
