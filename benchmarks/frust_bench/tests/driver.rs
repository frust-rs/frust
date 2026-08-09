//! Headless tests for the scenario driver + the two implemented scenarios
//! (S1, S7), driving Frust's `RenderRoot` directly (no GPU, no window) — the
//! same harness shape as `examples/huddle`'s own headless `tests/*.rs`.

use std::any::Any;

use frust::{AnyView, any, component};
use frust_core::{FrameTime, RenderRoot, View};
use frust_reactive::ReactiveRuntime;
use frust_scene::GlyphRun;
use frust_text::TextContext;
use frustbench::BenchApp;
use frustbench::scenarios::{self, BenchState, SCENARIOS, s1_animation::physics::bubble_count_for};
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target recording the workload: gradient/solid path fills,
/// strokes, glyph runs, and rounded-rect chrome (the HUD/switcher buttons).
#[derive(Default)]
struct RecScene {
    gradient_fills: usize,
    solid_fills: usize,
    strokes: usize,
    glyph_runs: usize,
    rounded: Vec<(Point, Size)>,
}

impl frust_core::PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
        self.rounded.push((origin, size));
    }
    fn fill_rounded_rect_brush(&mut self, origin: Point, size: Size, _radius: f64, _brush: &Brush) {
        self.rounded.push((origin, size));
    }
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, brush: &Brush) {
        match brush {
            Brush::Gradient(_) => self.gradient_fills += 1,
            _ => self.solid_fills += 1,
        }
    }
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {
        self.strokes += 1;
    }
}

/// Installs the reactive runtime + an ambient owner (required before creating
/// signals / mounting a Component).
fn setup() -> Owner {
    let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// One frame at `t_ms`: rebuild, layout (shaping real text), paint into a fresh
/// recorder. Returns the scene and whether paint asked for another frame.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

/// One frame the way Android's frame gate runs it: rebuild always, then lay out
/// **only** when the rebuild reports `needs_layout` (or `force_layout`, e.g. the
/// first frame), then paint. Returns the painted scene and whether layout
/// actually ran. See `docs/ARCHITECTURE.md`'s Frame gate ("Android also skips
/// layout unless `take_change_flags` reports `needs_layout()`, first frame, or
/// resize").
fn gated_frame<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
    force_layout: bool,
) -> (RecScene, bool) {
    let flags = root.rebuild(logic, state);
    let laid_out = force_layout || flags.needs_layout();
    if laid_out {
        let tcx_any: &mut dyn Any = tcx;
        root.layout_with_text(Size::new(W, H), tcx_any);
    }
    let mut scene = RecScene::default();
    root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, laid_out)
}

#[test]
fn registry_has_ten_scenarios_in_id_order() {
    // s1..=s10 (the frame-class table) stay in their original order and
    // indices unchanged, with d1/d2 (the DB op-latency class, PROTOCOL §9)
    // appended after — the registry widened from ten to twelve entries, but
    // nothing about the s1..=s10 slice moved.
    let ids: Vec<&str> = SCENARIOS.iter().map(|s| s.id()).collect();
    assert_eq!(
        ids,
        [
            "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "d1", "d2"
        ]
    );
}

#[test]
fn deep_link_and_id_parsing() {
    assert_eq!(scenarios::index_from_id("s1"), Some(0));
    assert_eq!(scenarios::index_from_id("s7"), Some(6));
    assert_eq!(scenarios::index_from_id("s9"), Some(8));
    // Two-digit ids match exactly — `s10` must never resolve as `s1`.
    assert_eq!(scenarios::index_from_id("s10"), Some(9));
    assert_eq!(scenarios::index_from_id("s11"), None);

    // The `d*` DB op-latency namespace (PROTOCOL §9.1) resolves through the
    // exact same opaque-string lookup as `s*` — nothing here special-cases
    // the `d`-prefix.
    assert_eq!(scenarios::index_from_id("d1"), Some(10));
    assert_eq!(scenarios::index_from_id("d2"), Some(11));
    assert_eq!(scenarios::index_from_id("d3"), None);
    assert_eq!(scenarios::index_from_url("frustbench://d1"), Some(10));
    assert_eq!(scenarios::index_from_url("frustbench://d2"), Some(11));

    assert_eq!(scenarios::index_from_url("frustbench://s1"), Some(0));
    assert_eq!(scenarios::index_from_url("frustbench://s7/idle"), Some(6));
    assert_eq!(scenarios::index_from_url("frustbench://s3?x=1"), Some(2));
    assert_eq!(scenarios::index_from_url("frustbench://nope"), None);
    assert_eq!(scenarios::index_from_url("other://s1"), None);

    // S9's profile rides a path segment on the same link (its only on-device
    // selection channel — see `scenarios::s9_terminal`'s module docs); the host
    // must still resolve to `s9`.
    assert_eq!(scenarios::index_from_url("frustbench://s9/htop"), Some(8));

    // S10 (the IME keystroke probe) rides the same host-is-the-id rule.
    assert_eq!(scenarios::index_from_url("frustbench://s10"), Some(9));
    assert_eq!(scenarios::index_from_url("frustbench://s1"), Some(0));
}

/// Drive one scenario's view over a `BenchState`, at a fixed index.
fn scenario_logic(idx: usize) -> impl FnMut(&mut BenchState) -> AnyView<BenchState> {
    move |state: &mut BenchState| SCENARIOS[idx].build(state)
}

#[test]
fn s1_paints_the_full_bubble_workload() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(0);

    // The bubble count is derived per play area (spec v3), not fixed — this
    // harness lays out at the driver's `W`x`H` (no SafeArea deflation here),
    // so the expected count is the same formula's output at that exact size.
    let expected_count = bubble_count_for(W, H);
    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert_eq!(
        scene.gradient_fills, expected_count,
        "one gradient fill per bubble"
    );
    assert_eq!(
        scene.strokes, expected_count,
        "one stroked border per bubble"
    );
    assert!(scene.glyph_runs >= expected_count, "shaped text per bubble");
    assert!(live, "a live animation storm keeps requesting frames");
}

#[test]
fn s7_is_static_and_paints_text() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(6); // s7

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        scene.glyph_runs > 0,
        "S7 paints its informational text (frame-gate idle demo)"
    );
    assert!(
        !live,
        "S7 requests no frames after the first — the frame gate goes idle"
    );
}

#[test]
fn every_registered_scenario_mounts_and_paints() {
    // All of S1-S8 are implemented (tasks 02/03/04) — no stubs remain.
    // Each must mount from the registry and paint something on frame 0;
    // per-scenario behavior (animation liveness, scroll, decode) is covered
    // by the dedicated tests in this file and the scenario modules.
    //
    // **S9 (index 8) is deliberately outside this loop.** Its whole contract is
    // that a frame happens only when its 30Hz feed loop publishes a generation,
    // and that loop needs a `pump_local` this harness never performs — so at
    // mount S9 correctly paints only its backdrop (a `fill_rect`, which
    // `RecScene` does not record) and requests nothing. "Inert at mount" is the
    // `idle` profile's rest-cost measurement, not a defect; S9's own coverage is
    // in `scenarios::s9_terminal`'s unit tests.
    //
    // **S10 (index 9) is outside it too**, for the same shape of reason: it is a
    // touch-driven IME probe, so its whole surface is exercised by
    // `scenarios::s10_keys`' own unit tests (which mount it in a `RenderRoot`
    // and dispatch real pointer/IME events) rather than by a mount-and-paint
    // sweep.
    for idx in 0..8 {
        let _owner = setup();
        let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
        let mut state = BenchState::new();
        let mut tcx = TextContext::new();
        let mut logic = scenario_logic(idx);

        let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        // A scenario must not be inert at mount: it either paints something
        // on frame 0 or is live (requesting frames while async work — e.g.
        // S5's first image decode — completes).
        assert!(
            scene.glyph_runs > 0 || scene.solid_fills > 0 || scene.gradient_fills > 0 || live,
            "scenario index {idx} is inert on its first frame (paints nothing, requests nothing)"
        );
    }
}

#[test]
fn s2_long_list_scrolls_and_keeps_requesting_frames() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(1); // s2

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        scene.glyph_runs > 0,
        "S2 paints row title/subtitle text for the materialized window"
    );
    assert!(
        live,
        "S2's scripted auto-scroll keeps requesting frames (see s2_list's module doc)"
    );
}

#[test]
fn s5_image_pipeline_keeps_requesting_frames() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(4); // s5

    let (_scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        live,
        "S5's scripted auto-scroll keeps requesting frames while decodes stream in"
    );
}

#[test]
fn s2_scene_stays_nonempty_under_android_layout_skip_gate() {
    // Regression for the on-device empty-render bug: S2's materialized row
    // window (and shaped-text cache) is computed only in `layout`, while the
    // scripted scroll offset advances only in `paint`. Under Android's frame
    // gate, layout runs only when the rebuild reports `needs_layout`; if S2's
    // rebuild reported only `PAINT` (the bug), layout would be skipped after
    // frame 1, the window would freeze at offset 0, and the scripted scroll
    // would slide every row off-screen into an empty scene. Here we drive the
    // exact gate: layout only on frame 1 and thereafter only when the rebuild
    // asks for it, and assert the scene stays non-empty as the scroll advances.
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(1); // s2

    let (first, laid1) = gated_frame(&mut root, &mut logic, &mut state, &mut tcx, 0, true);
    assert!(laid1, "frame 1 always lays out (first build)");
    assert!(first.glyph_runs > 0, "S2 paints row text on frame 1");

    // Drive several seconds of scripted scroll at 300ms/frame. The fling phase
    // pushes the offset far past the frame-1 window, so a frozen window would
    // paint zero rows here.
    let mut saw_gated_relayout = false;
    let mut min_glyphs = usize::MAX;
    for i in 1..=40u64 {
        let (scene, laid) =
            gated_frame(&mut root, &mut logic, &mut state, &mut tcx, i * 300, false);
        if laid {
            saw_gated_relayout = true;
        }
        min_glyphs = min_glyphs.min(scene.glyph_runs);
    }
    assert!(
        saw_gated_relayout,
        "S2 must report needs_layout as its scripted scroll advances, so the \
         Android gate relayouts and the row window follows the scroll"
    );
    assert!(
        min_glyphs > 0,
        "S2's scene must stay non-empty across the scripted scroll under the \
         layout-skip gate (got an empty frame — the on-device regression)"
    );
}

#[test]
fn s5_requests_relayout_as_scroll_advances_under_layout_skip_gate() {
    // S5 has the same shape as S2 (window + per-cell `Image` widgets built only
    // in `layout`, offset advanced only in `paint`) plus decode scheduling keyed
    // off the layout-reported `visible_ids`. Its scene draws via `draw_image`/
    // `fill_rect`, which the GPU-free `RecScene` records as nothing, so we assert
    // the load-bearing contract directly: the widget reports `needs_layout` as
    // the scroll advances, so the Android gate relayouts (re-windowing the cells
    // and republishing `visible_ids` so new images decode). If it reported only
    // `PAINT` (the bug), the window would freeze after frame 1 and the stream
    // would render nothing on device.
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(4); // s5

    let (_first, laid1) = gated_frame(&mut root, &mut logic, &mut state, &mut tcx, 0, true);
    assert!(laid1, "frame 1 always lays out (first build)");

    let mut saw_gated_relayout = false;
    for i in 1..=20u64 {
        let (_scene, laid) =
            gated_frame(&mut root, &mut logic, &mut state, &mut tcx, i * 300, false);
        if laid {
            saw_gated_relayout = true;
        }
    }
    assert!(
        saw_gated_relayout,
        "S5 must report needs_layout as its scripted scroll advances, so the \
         Android gate relayouts (cell window + decode-scheduling visible_ids \
         follow the scroll instead of freezing into an empty stream)"
    );
}

#[test]
fn s3_cycles_continuously_and_repopulates_after_each_clear_under_layout_skip_gate() {
    // Regression companion to the S2/S5 gated tests, for S3's *op-driven*
    // mutations, extended for the continuous-cycling redesign (S3 no longer
    // runs its op sequence once — it repeats create1k → create10k → update →
    // swap → clear indefinitely; see `scenarios/s3_table.rs`'s module doc).
    //
    // Unlike the pre-fix S2/S5 (whose changing quantity — the scroll offset —
    // was advanced only in `paint`, so their rebuild reported only `PAINT` and
    // the Android layout-skip gate froze the window into emptiness), S3
    // mutates its row set inside the component's `build`, and the virtualized
    // `list_view`'s reconcile already reports `ChangeFlags::LAYOUT` on every
    // op (a count change on create/clear, an in-place text re-shape on
    // update/swap). The on-device capture confirms this: every S3 op frame
    // relayouts (`layout_us` > 0 — create1k 317µs, create10k 435µs, update
    // 1260µs, swap 2563µs, clear 395µs), so S3 does NOT have the S2/S5
    // layout-gate disease.
    //
    // This test locks in the continuous-cycling contract under the exact gate
    // (layout only on frame 1, and thereafter only when the rebuild asks):
    // the scene goes empty at every cycle's terminal `clear` AND repopulates
    // when the next cycle's `create1k` starts — repeatedly, more than once
    // within the run window — while every op (including each cycle's own
    // `create1k`) forces a gated relayout. A regression that made an op
    // report only `PAINT` would blank/stale the table mid-op instead of at a
    // legitimate `clear`/`create1k` boundary (the "glitches out" signature).
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(2); // s3

    let (first, laid1) = gated_frame(&mut root, &mut logic, &mut state, &mut tcx, 0, true);
    assert!(laid1, "frame 1 always lays out (first build)");
    assert!(first.glyph_runs > 0, "S3 paints its first rows on frame 1");

    let mut relayouts = 0usize;
    let mut empty_episodes = 0usize;
    let mut refills = 0usize;
    let mut prev_empty = false; // frame 1 (above) was non-empty
    // Drive well past several scripted cycles (5 ops × ~20 settle frames ≈
    // 100 rebuilds per cycle); the script advances by rebuild count, not wall
    // time, so 300 frames comfortably covers 2+ full cycles beyond frame 1.
    for i in 1..=300u64 {
        let (scene, laid) = gated_frame(&mut root, &mut logic, &mut state, &mut tcx, i * 16, false);
        if laid {
            relayouts += 1;
        }
        let now_empty = scene.glyph_runs == 0;
        if now_empty && !prev_empty {
            empty_episodes += 1;
        }
        if !now_empty && prev_empty {
            refills += 1;
        }
        prev_empty = now_empty;
    }

    assert!(
        empty_episodes >= 2,
        "the table must go empty at the terminal `clear` of every cycle — at \
         least twice within this run window (continuous cycling), got \
         {empty_episodes}"
    );
    assert!(
        refills >= 2,
        "the table must repopulate after each cycle's clear — the next \
         cycle's create1k must refill the scene (continuous cycling), got \
         {refills}"
    );
    assert!(
        relayouts >= 9,
        "each S3 op every cycle (create10k/update/swap/clear after the \
         first-frame create1k, then all five ops of at least one full \
         subsequent cycle) must report needs_layout so the Android gate \
         relayouts and the visible rows reflect the mutation (got \
         {relayouts} gated relayouts)"
    );
}

#[test]
fn full_app_mounts_with_scenario_switcher() {
    let _owner = setup();
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(BenchApp));
    let mut state = ();
    let mut tcx = TextContext::new();

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(live, "the mounted app starts on S1 (animating)");
    // S1's own HUD paints two buttons (Pause/Reset) plus the eight-scenario
    // switcher row: at least ten rounded-rect buttons in total.
    assert!(
        scene.rounded.len() >= SCENARIOS.len() + 2,
        "the switcher paints one button per scenario plus S1's HUD (got {})",
        scene.rounded.len()
    );
}
