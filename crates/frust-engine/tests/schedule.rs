//! What a recorded layer shape costs in render passes, and what the scheduler
//! refuses rather than serving wrong.
//!
//! Every case drives [`Schedule::build`] over a recording built by hand through
//! `vello_common`'s own [`CommandRecorder`] — the same structure the engine's
//! compiler fills in — because the scheduler's whole input is that recording's
//! shape. No GPU, device, surface or texture is involved anywhere in this file:
//! a page is a decision about a group and an extent, and the pool that hands the
//! texture out is not consulted until execute time.
//!
//! The four claims under test are the ones the scheduler exists for: a frame
//! with no isolated layer costs exactly one pass, each isolated layer costs one
//! more, a nested chain ping-pongs between two texture groups however deep it
//! runs, and a shape needing a third live page is refused with a reason a log
//! can be read from rather than rendered incorrectly.

use frust_engine::schedule::{
    MAX_CHAIN_DEPTH, MAX_LIVE_PAGES, PageConfig, PageParity, Round, RoundOp, Schedule, pages,
};
use frust_engine::{EngineDraw, EngineError};
use frust_gpu::{DownlevelProfile, TierCaps};
use vello_common::color::palette::css::RED;
use vello_common::geometry::RectU16;
use vello_common::paint::Paint;
use vello_common::peniko::{BlendMode, Compose, Mix};
use vello_common::record::{CommandRecorder, LayerProps};
use vello_common::strip::Strip;
use vello_common::tile::Tile;

/// Viewport every recording is built against. Deliberately not square, so an
/// axis swapped somewhere in page sizing cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (256, 192);

/// The opacity every isolated layer in this file is recorded at — strictly
/// between transparent and opaque, which is what makes a layer isolating.
const HALF: f32 = 0.5;

fn caps() -> TierCaps {
    TierCaps::fake(DownlevelProfile::Full)
}

fn recorder() -> CommandRecorder<EngineDraw> {
    CommandRecorder::new(VIEWPORT.0, VIEWPORT.1)
}

fn schedule(recorder: &CommandRecorder<EngineDraw>) -> Vec<Round> {
    Schedule::build(recorder, &caps(), &PageConfig::default()).expect("a chain schedules")
}

fn escalation(recorder: &CommandRecorder<EngineDraw>) -> String {
    match Schedule::build(recorder, &caps(), &PageConfig::default()) {
        Err(EngineError::SchedulerEscalation { reason }) => reason,
        other => panic!("expected an escalation, got {other:?}"),
    }
}

/// A regular layer composited source-over at `opacity`, with no mask and no
/// layer clip path — the only layer shape frust's display list records.
fn layer(opacity: f32) -> LayerProps {
    LayerProps {
        blend_mode: BlendMode::default(),
        opacity,
        mask: None,
        clip_path: None,
    }
}

/// Strips covering `width` pixels of the tile row holding `y`, starting at `x`.
///
/// A strip run is read pairwise — each strip's width is the distance to the
/// next one's alpha index — so the run is a strip plus the sentinel that closes
/// it, and the sentinel's index is what sets the width.
fn strips(x: u16, y: u16, width: u16) -> Vec<Strip> {
    vec![
        Strip::new(x, y, 0, false),
        Strip::sentinel(y, u32::from(width) * u32::from(Tile::HEIGHT)),
    ]
}

/// Records one draw covering `width` pixels of the tile row holding `y`.
fn draw(recorder: &mut CommandRecorder<EngineDraw>, x: u16, y: u16, width: u16) {
    let strips = strips(x, y, width);
    let depth = u32::try_from(recorder.draws.len()).expect("a test records a handful of draws");
    recorder.push_draw(
        EngineDraw::new(Paint::from(RED), depth, 0..strips.len()),
        &strips,
    );
}

/// A recording of `depth` nested isolated layers, the innermost holding one
/// draw.
fn nested_chain(depth: usize) -> CommandRecorder<EngineDraw> {
    let mut recorder = recorder();
    for _ in 0..depth {
        recorder.push_layer(layer(HALF), None);
    }
    draw(&mut recorder, 16, 16, 32);
    for _ in 0..depth {
        recorder.pop_layer();
    }
    recorder
}

/// The draw ranges a round issues, in execution order.
fn draw_ranges(round: &Round) -> Vec<(u32, u32)> {
    round
        .ops
        .iter()
        .filter_map(|op| match op {
            RoundOp::Draws(range) => Some((range.start, range.end)),
            RoundOp::Composite(_) => None,
        })
        .collect()
}

// ---------------------------------------------------------------------
// A frame with no isolated layer is one pass
// ---------------------------------------------------------------------

#[test]
fn a_recording_with_no_layers_schedules_to_exactly_one_round() {
    let mut recorder = recorder();
    for i in 0..3 {
        draw(&mut recorder, 16 * i, 16, 32);
    }

    let rounds = schedule(&recorder);

    assert_eq!(
        rounds.len(),
        1,
        "a clip-only frame costs one render pass and no intermediate: {rounds:?}"
    );
    assert!(rounds[0].is_root());
    assert!(rounds[0].page().is_none());
    assert_eq!(rounds[0].draw_count(), 3);
    assert_eq!(draw_ranges(&rounds[0]), vec![(0, 3)]);
    assert!(rounds[0].released.is_empty());
    assert_eq!(rounds[0].composites().count(), 0);
}

#[test]
fn an_empty_recording_still_schedules_to_the_root_round() {
    let rounds = schedule(&recorder());

    assert_eq!(rounds.len(), 1);
    assert!(rounds[0].is_root());
    assert!(rounds[0].ops.is_empty());
    assert_eq!(rounds[0].draw_count(), 0);
}

// ---------------------------------------------------------------------
// Each isolated layer is one more pass
// ---------------------------------------------------------------------

#[test]
fn one_opacity_layer_schedules_to_two_rounds() {
    let mut recorder = recorder();
    draw(&mut recorder, 0, 0, 16);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    draw(&mut recorder, 64, 32, 16);

    let rounds = schedule(&recorder);
    assert_eq!(
        rounds.len(),
        2,
        "one layer costs one extra pass: {rounds:?}"
    );

    // The layer is rendered first, into a page of its own.
    let page = rounds[0].page().expect("the layer round targets a page");
    assert_eq!(page.layer, 0);
    assert_eq!(page.depth, 1);
    assert_eq!(page.parity, PageParity::Odd);
    assert_eq!(page.bounds, RectU16::new(16, 16, 48, 20));
    assert_eq!(
        page.size,
        pages::PageSize {
            width: pages::DEFAULT_MIN_PAGE_SIZE,
            height: pages::DEFAULT_MIN_PAGE_SIZE
        },
        "a small layer is floored at the minimum page size rather than sized to its bounds"
    );
    assert_eq!(draw_ranges(&rounds[0]), vec![(1, 2)]);
    assert!(rounds[0].released.is_empty());

    // The surface follows, with the composite spliced in exactly where the
    // recording entered the layer.
    assert!(rounds[1].is_root());
    assert_eq!(draw_ranges(&rounds[1]), vec![(0, 1), (2, 3)]);
    assert!(matches!(
        rounds[1].ops.as_slice(),
        [RoundOp::Draws(_), RoundOp::Composite(_), RoundOp::Draws(_)]
    ));
    assert_eq!(rounds[1].released, vec![PageParity::Odd]);

    let composite = rounds[1]
        .composites()
        .next()
        .expect("the root round composites the layer");
    assert_eq!(composite.layer, 0);
    assert_eq!(composite.parity, PageParity::Odd);
    assert_eq!(composite.opacity, HALF);
    assert_eq!(composite.bounds, page.bounds);
    // The layer was rendered at the page's origin, so the sampled region is its
    // bounds moved there.
    assert_eq!(composite.source(), RectU16::new(0, 0, 32, 4));
}

// ---------------------------------------------------------------------
// A nested chain ping-pongs between two texture groups
// ---------------------------------------------------------------------

#[test]
fn a_depth_four_chain_alternates_page_groups_and_ends_at_the_surface() {
    let rounds = schedule(&nested_chain(4));

    assert_eq!(rounds.len(), 5, "four layers plus the surface: {rounds:?}");

    let pages: Vec<(usize, PageParity)> = rounds
        .iter()
        .filter_map(|round| round.page().map(|page| (page.depth, page.parity)))
        .collect();
    assert_eq!(
        pages,
        vec![
            (4, PageParity::Even),
            (3, PageParity::Odd),
            (2, PageParity::Even),
            (1, PageParity::Odd),
        ],
        "rounds run innermost-first and the group alternates with depth"
    );

    assert!(rounds[4].is_root());
    assert_eq!(rounds[4].released, vec![PageParity::Odd]);

    // The innermost layer holds the frame's only draw; every round above it
    // does nothing but composite the one below.
    assert_eq!(draw_ranges(&rounds[0]), vec![(0, 1)]);
    for round in &rounds[1..] {
        assert_eq!(draw_ranges(round), Vec::<(u32, u32)>::new());
        assert_eq!(round.composites().count(), 1);
    }
}

#[test]
fn a_chain_never_holds_more_than_two_pages_live_at_once() {
    for depth in 1..=MAX_CHAIN_DEPTH {
        for round in schedule(&nested_chain(depth)) {
            let live = usize::from(round.page().is_some()) + round.released.len();
            assert!(
                live <= MAX_LIVE_PAGES,
                "depth {depth} holds {live} pages live in {round:?}"
            );
            if let Some(page) = round.page() {
                assert!(
                    !round.released.contains(&page.parity),
                    "a round samples a page from the group it renders into: {round:?}"
                );
                assert_eq!(
                    round.released,
                    if depth > page.depth {
                        vec![page.parity.opposite()]
                    } else {
                        Vec::new()
                    },
                    "a layer samples only its child's page, from the other group"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// Layers that need no page of their own
// ---------------------------------------------------------------------

#[test]
fn a_fully_opaque_layer_is_inlined_rather_than_given_a_page() {
    let mut recorder = recorder();
    draw(&mut recorder, 0, 0, 16);
    recorder.push_layer(layer(1.0), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    draw(&mut recorder, 64, 32, 16);

    let rounds = schedule(&recorder);

    assert_eq!(
        rounds.len(),
        1,
        "compositing at full opacity is drawing the contents directly: {rounds:?}"
    );
    assert!(rounds[0].is_root());
    assert_eq!(rounds[0].draw_count(), 3);
    assert_eq!(draw_ranges(&rounds[0]), vec![(0, 1), (1, 2), (2, 3)]);
    assert_eq!(rounds[0].composites().count(), 0);
}

#[test]
fn an_inlined_layer_does_not_consume_a_depth_level() {
    // Two isolated layers with an opaque one wedged between them: the isolated
    // pair still lands on opposite groups, which is what keeps two live pages
    // out of the same group.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(1.0), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();
    recorder.pop_layer();

    let rounds = schedule(&recorder);

    let pages: Vec<(usize, PageParity)> = rounds
        .iter()
        .filter_map(|round| round.page().map(|page| (page.depth, page.parity)))
        .collect();
    assert_eq!(pages, vec![(2, PageParity::Even), (1, PageParity::Odd)]);
    assert_eq!(rounds.len(), 3);
}

#[test]
fn a_fully_transparent_layer_and_everything_inside_it_is_dropped() {
    let mut recorder = recorder();
    draw(&mut recorder, 0, 0, 16);
    recorder.push_layer(layer(0.0), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 32, 32, 32);
    recorder.pop_layer();
    recorder.pop_layer();
    draw(&mut recorder, 64, 64, 16);

    let rounds = schedule(&recorder);

    assert_eq!(
        rounds.len(),
        1,
        "nothing inside a zero-opacity layer reaches the frame: {rounds:?}"
    );
    assert!(rounds[0].is_root());
    assert_eq!(draw_ranges(&rounds[0]), vec![(0, 1), (3, 4)]);
    assert_eq!(rounds[0].composites().count(), 0);
}

#[test]
fn a_layer_covering_nothing_costs_no_pass() {
    let mut recorder = recorder();
    draw(&mut recorder, 0, 0, 16);
    recorder.push_layer(layer(HALF), None);
    recorder.pop_layer();

    let rounds = schedule(&recorder);

    assert_eq!(rounds.len(), 1);
    assert!(rounds[0].is_root());
    assert_eq!(rounds[0].composites().count(), 0);
}

// ---------------------------------------------------------------------
// Shapes the scheduler refuses
// ---------------------------------------------------------------------

#[test]
fn a_branching_layer_graph_escalates_with_a_readable_reason() {
    // The shape that needs a third live page: a parent with two children, the
    // second of which has a child of its own. While the second child is
    // scheduled the parent's page is still holding the first child's result.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 64, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();
    recorder.pop_layer();

    let reason = escalation(&recorder);

    assert!(
        reason.contains("layer 0 enters 2 child layers"),
        "the reason names the branch it found: {reason}"
    );
    assert!(
        reason.contains("third live intermediate page"),
        "the reason says why a branch is refused: {reason}"
    );
}

#[test]
fn a_branch_at_the_root_escalates_too() {
    let mut recorder = recorder();
    for _ in 0..2 {
        recorder.push_layer(layer(HALF), None);
        draw(&mut recorder, 16, 16, 32);
        recorder.pop_layer();
    }

    let reason = escalation(&recorder);
    assert!(
        reason.contains("the root enters 2 child layers"),
        "the reason names the root as the branch point: {reason}"
    );
}

#[test]
fn a_chain_deeper_than_the_scheduler_serves_escalates() {
    let reason = escalation(&nested_chain(MAX_CHAIN_DEPTH + 1));

    assert!(
        reason.contains("nested isolated layers"),
        "the reason names the chain it found: {reason}"
    );
    assert!(
        reason.contains(&MAX_CHAIN_DEPTH.to_string()),
        "the reason names the depth it serves: {reason}"
    );
}

#[test]
fn a_non_default_blend_mode_escalates() {
    let mut recorder = recorder();
    recorder.push_layer(
        LayerProps {
            blend_mode: BlendMode::new(Mix::Multiply, Compose::SrcOver),
            opacity: 1.0,
            mask: None,
            clip_path: None,
        },
        None,
    );
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();

    let reason = escalation(&recorder);
    assert!(
        reason.contains("non-default blend mode"),
        "the reason names the blend mode it found: {reason}"
    );
}

#[test]
fn a_non_finite_opacity_escalates_rather_than_sizing_a_page_against_it() {
    let mut recorder = recorder();
    recorder.push_layer(layer(f32::NAN), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();

    let reason = escalation(&recorder);
    assert!(
        reason.contains("non-finite opacity"),
        "the reason names the opacity it found: {reason}"
    );
}

#[test]
fn a_layer_larger_than_a_page_is_refused_rather_than_clipped_to_the_ceiling() {
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 0, 128);
    recorder.pop_layer();

    let config = PageConfig {
        min_page_size: 64,
        max_page_size: 64,
    };
    assert_eq!(pages::page_ceiling(&config, &caps()), 64);

    assert!(
        matches!(
            Schedule::build(&recorder, &caps(), &config),
            Err(EngineError::IntermediateTextureTooLarge)
        ),
        "a layer wider than the ceiling is refused, not shrunk onto it"
    );

    // The same recording schedules under the default bounds, so the refusal is
    // the ceiling's doing and not the recording's.
    assert_eq!(schedule(&recorder).len(), 2);
}

#[test]
fn a_refused_frame_never_reports_a_target_it_would_have_rendered() {
    // Every escalation path returns the error rather than a partial schedule:
    // a caller that took a `Vec<Round>` from a refused frame would render a
    // layer graph it had already been told the scheduler cannot serve.
    let recorders = [nested_chain(MAX_CHAIN_DEPTH + 1), {
        let mut recorder = recorder();
        for _ in 0..2 {
            recorder.push_layer(layer(HALF), None);
            draw(&mut recorder, 16, 16, 32);
            recorder.pop_layer();
        }
        recorder
    }];

    for recorder in &recorders {
        assert!(Schedule::build(recorder, &caps(), &PageConfig::default()).is_err());
    }
}

#[test]
fn an_escalation_reads_as_a_scheduler_escalation_with_its_reason_attached() {
    let error = EngineError::SchedulerEscalation {
        reason: escalation(&nested_chain(MAX_CHAIN_DEPTH + 1)),
    };
    let rendered = error.to_string();

    assert!(rendered.starts_with("scheduler escalation: "));
    assert!(rendered.contains("nested isolated layers"));
}
