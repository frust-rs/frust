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
//! The five claims under test are the ones the scheduler exists for: a frame
//! with no isolated layer costs exactly one pass, each isolated layer costs one
//! more, a nested chain ping-pongs between two texture groups however deep it
//! runs, sibling layers beside each other take the two groups one apiece rather
//! than overwriting one another, and a shape needing a third live page is
//! refused with a reason a log can be read from rather than rendered
//! incorrectly.

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

/// A recording of `count` isolated layers side by side under the root, each
/// holding one draw of its own so no two share a page's bounds.
fn sibling_fan(count: u16) -> CommandRecorder<EngineDraw> {
    let mut recorder = recorder();
    for index in 0..count {
        recorder.push_layer(layer(HALF), None);
        draw(&mut recorder, index.saturating_mul(48), 16, 32);
        recorder.pop_layer();
    }
    recorder
}

/// Walks `rounds` the way the renderer executes them and fails if any round
/// would sample a page an earlier round had already overwritten.
///
/// One slot per ping-pong group, which is the bound itself: the scheduler hands
/// out a parity, and a parity *is* a page identity for the shapes it serves, so
/// a schedule that put two live pages in one group shows up here as a round
/// rendering over a page still owed to a later composite.
fn assert_pages_survive_until_composited(rounds: &[Round]) {
    let mut live: [Option<u32>; MAX_LIVE_PAGES] = [None; MAX_LIVE_PAGES];

    for round in rounds {
        for composite in round.composites() {
            assert_eq!(
                live[composite.parity.index()],
                Some(composite.layer),
                "a round composites layer {} out of the {:?} group, which holds {:?}: {round:?}",
                composite.layer,
                composite.parity,
                live[composite.parity.index()]
            );
        }

        // The round's own page is live for the whole of its pass, alongside
        // every page that pass samples, so the group has to be free first.
        if let Some(page) = round.page() {
            assert!(
                live[page.parity.index()].is_none(),
                "a round renders into the {:?} group over a page a later round still \
                 composites: {round:?}",
                page.parity
            );
        }

        for parity in &round.released {
            live[parity.index()] = None;
        }
        if let Some(page) = round.page() {
            live[page.parity.index()] = Some(page.layer);
        }
    }
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
// Sibling layers take the two groups one apiece
// ---------------------------------------------------------------------

#[test]
fn two_sibling_opacity_layers_schedule_to_three_rounds() {
    // Two widgets fading at the same time — a nav crossfade, a list row
    // dismissing beside another — is the commonest sibling shape a frust screen
    // records, and it costs one pass per layer plus the surface.
    let rounds = schedule(&sibling_fan(2));

    assert_eq!(rounds.len(), 3, "a page each, then the surface: {rounds:?}");

    let pages: Vec<(u32, usize, PageParity)> = rounds
        .iter()
        .filter_map(|round| {
            round
                .page()
                .map(|page| (page.layer, page.depth, page.parity))
        })
        .collect();
    assert_eq!(
        pages,
        vec![(0, 1, PageParity::Odd), (1, 1, PageParity::Even)],
        "siblings share a depth and so a preferred group; the second takes the one left free"
    );
    assert_eq!(draw_ranges(&rounds[0]), vec![(0, 1)]);
    assert_eq!(draw_ranges(&rounds[1]), vec![(1, 2)]);
    assert!(
        rounds[..2].iter().all(|round| round.released.is_empty()),
        "neither sibling samples the other: {rounds:?}"
    );

    // The surface composites both, in the order the recording entered them.
    assert!(rounds[2].is_root());
    let composited: Vec<u32> = rounds[2].composites().map(|c| c.layer).collect();
    assert_eq!(composited, vec![0, 1]);
    assert_eq!(rounds[2].released, vec![PageParity::Odd, PageParity::Even]);
    assert_pages_survive_until_composited(&rounds);
}

#[test]
fn a_sibling_fan_keeps_each_layers_own_bounds_and_recording_order() {
    let rounds = schedule(&sibling_fan(2));

    let bounds: Vec<RectU16> = rounds
        .iter()
        .filter_map(|round| round.page().map(|page| page.bounds))
        .collect();
    assert_eq!(
        bounds,
        vec![RectU16::new(0, 16, 32, 20), RectU16::new(48, 16, 80, 20)],
        "each sibling's page is sized and placed from its own contents"
    );

    for composite in rounds[2].composites() {
        let page = rounds
            .iter()
            .filter_map(Round::page)
            .find(|page| page.layer == composite.layer)
            .expect("every composited layer has a page round of its own");
        assert_eq!(composite.bounds, page.bounds);
        assert_eq!(composite.parity, page.parity);
        assert_eq!(composite.opacity, HALF);
    }
}

#[test]
fn a_flat_sibling_fits_beside_a_chain_that_has_climbed_back_to_one_page() {
    // A chain holds both groups while it runs, and one once it reaches its
    // outermost layer — which is exactly what leaves room for a sibling beside
    // it. The chain is scheduled first because the recording entered it first.
    let mut recorder = recorder();
    for _ in 0..MAX_CHAIN_DEPTH {
        recorder.push_layer(layer(HALF), None);
    }
    draw(&mut recorder, 16, 16, 32);
    for _ in 0..MAX_CHAIN_DEPTH {
        recorder.pop_layer();
    }
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 96, 16, 32);
    recorder.pop_layer();

    let rounds = schedule(&recorder);

    assert_eq!(
        rounds.len(),
        MAX_CHAIN_DEPTH + 2,
        "one pass per chain link, one for the sibling, one for the surface: {rounds:?}"
    );
    assert_pages_survive_until_composited(&rounds);

    let root = rounds
        .last()
        .expect("a schedule always ends at the surface");
    assert!(root.is_root());
    let composited: Vec<u32> = root.composites().map(|c| c.layer).collect();
    assert_eq!(
        composited,
        vec![0, MAX_CHAIN_DEPTH as u32],
        "the surface composites the chain's outermost layer and the sibling beside it"
    );
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
fn a_sibling_fan_wider_than_the_two_groups_escalates_with_a_readable_reason() {
    // Three siblings under the root need three pages live at the one pass that
    // composites them all, and there are two groups to take them from.
    let reason = escalation(&sibling_fan(3));

    assert!(
        reason.contains("layer 2"),
        "the reason names the sibling that found no group: {reason}"
    );
    assert!(
        reason.contains("third live intermediate page"),
        "the reason says what ran out: {reason}"
    );
}

#[test]
fn a_branch_whose_second_subtree_nests_escalates() {
    // A parent with two children, the second of which has a child of its own:
    // scheduling that grandchild takes the group the first child's finished
    // page is in, and the parent then has none left for its own.
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
        reason.contains("layer 2"),
        "the reason names the layer that found no group: {reason}"
    );
    assert!(
        reason.contains("third live intermediate page"),
        "the reason says what ran out: {reason}"
    );
}

#[test]
fn a_sibling_needing_both_groups_beside_a_finished_page_escalates() {
    // The mirror of the shape that schedules: a flat sibling first, then one
    // that nests. The nesting sibling needs both groups at once, and the flat
    // one's page is still owed to the surface, so there is only one to have.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 64, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();

    let reason = escalation(&recorder);

    assert!(
        reason.contains("layer 1"),
        "the reason names the sibling that found no group: {reason}"
    );
    assert!(
        reason.contains("third live intermediate page"),
        "the reason says what ran out: {reason}"
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
    let recorders = [nested_chain(MAX_CHAIN_DEPTH + 1), sibling_fan(3)];

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
