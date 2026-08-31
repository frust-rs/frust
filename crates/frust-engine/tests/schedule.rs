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
//! The claims under test are the ones the scheduler exists for: a frame with no
//! isolated layer costs exactly one pass, each isolated layer costs one more, a
//! nested chain ping-pongs between two texture groups however deep it runs,
//! sibling layers beside each other take the two groups one apiece and a fan
//! wider than that is served by cutting its parent's round short rather than by
//! a third group, a layer whose own round samples a live page while an isolated
//! ancestor holds another is served on the one spill page beside the pair, and a
//! shape that still finds no page after a cut and the spill is refused with a
//! reason a log can be read from rather than rendered incorrectly.

use frust_engine::schedule::{
    MAX_CHAIN_DEPTH, MAX_LIVE_PAGES, PING_PONG_GROUPS, PageConfig, PageParity, Round, RoundOp,
    Schedule, pages,
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

/// A recording of one isolated parent holding a flat isolated child and then a
/// nesting one — the smallest shape the two ping-pong groups cannot serve, and
/// the shape the spill page exists for.
///
/// The parent takes a group of its own once its round is cut after the first
/// child, the grandchild takes the other, and the second child — whose own
/// round has to sample the grandchild's page — finds neither free.
///
/// This is the transition shape in miniature: `parent` is the page being
/// scrubbed, the first child a chip beside the one that carries a translucent
/// chip of its own.
fn branching_chain() -> CommandRecorder<EngineDraw> {
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
    recorder
}

/// [`branching_chain`]'s own layers recorded at the root instead of inside a
/// parent: a flat isolated layer, then one that nests.
///
/// The contents are the same and the nesting is the same; what is gone is the
/// ancestor holding a page across the whole walk. It is the control for the
/// spill cases — the shape a transition records at rest, which the two groups
/// have always served.
fn branching_chain_at_the_root() -> CommandRecorder<EngineDraw> {
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 64, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();
    recorder
}

/// A recording of a sibling pair nested inside a sibling pair — the smallest
/// shape three live pages cannot serve either.
///
/// The outer parent takes a group at the cut its first child forces, the inner
/// parent's own two children take the other group and the spill page between
/// them, and the inner parent — whose round composites *both* of them — would
/// need a fourth live page to render into.
fn nested_fan_inside_a_fan() -> CommandRecorder<EngineDraw> {
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 48, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 96, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();
    recorder.pop_layer();
    recorder
}

/// Walks `rounds` the way the renderer executes them and fails if any round
/// would sample a page an earlier round had already overwritten.
///
/// One slot per live page — the two ping-pong groups and the spill page — which
/// is the bound itself: the scheduler hands out a group, and a group *is* a page
/// identity for the shapes it serves, so a schedule that put two live pages in
/// one group shows up here as a round rendering over a page still owed to a
/// later composite. The spill page is checked by exactly the same rules as the
/// pair, which is the claim that it is released like any other page rather than
/// held for the frame. A continuation round
/// is the one round allowed to render into an occupied group, and only into its
/// own layer's page — the whole point of the `continued` flag is that the
/// renderer loads that page instead of clearing it.
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
        // every page that pass samples, so the group has to be free first —
        // unless this round is continuing the page it already holds there.
        if let Some(page) = round.page() {
            if page.continued {
                assert_eq!(
                    live[page.parity.index()],
                    Some(page.layer),
                    "a round continues a page the {:?} group is not holding: {round:?}",
                    page.parity
                );
            } else {
                assert!(
                    live[page.parity.index()].is_none(),
                    "a round renders into the {:?} group over a page a later round still \
                     composites: {round:?}",
                    page.parity
                );
            }
        }

        for parity in &round.released {
            live[parity.index()] = None;
        }
        if let Some(page) = round.page() {
            live[page.parity.index()] = Some(page.layer);
        }
    }
}

/// The layers `rounds` composites, in execution order, paired with the round
/// each composite happens in.
fn composite_order(rounds: &[Round]) -> Vec<(usize, u32)> {
    rounds
        .iter()
        .enumerate()
        .flat_map(|(index, round)| round.composites().map(move |c| (index, c.layer)))
        .collect()
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
                live <= PING_PONG_GROUPS,
                "depth {depth} holds {live} pages live in {round:?}"
            );
            assert!(
                !round
                    .page()
                    .is_some_and(|page| page.parity == PageParity::Spill),
                "a chain alternates between the pair and never reaches for the spill page: \
                 {round:?}"
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

#[test]
fn nothing_inside_a_layer_covering_nothing_costs_a_pass_either() {
    // A layer covering no pixels composites nothing, so no round of its own is
    // emitted — and neither is any round its children would have rendered,
    // whose pages nothing would ever have sampled. They used to be scheduled
    // and then thrown away with the parent that would have read them.
    let mut recorder = recorder();
    draw(&mut recorder, 0, 0, 16);
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    recorder.pop_layer();
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    recorder.pop_layer();
    recorder.pop_layer();
    draw(&mut recorder, 64, 64, 16);

    let rounds = schedule(&recorder);

    assert_eq!(
        rounds.len(),
        1,
        "an empty layer's whole subtree is pruned, not rendered into pages: {rounds:?}"
    );
    assert!(rounds[0].is_root());
    assert_eq!(rounds[0].composites().count(), 0);
    assert_eq!(draw_ranges(&rounds[0]), vec![(0, 1), (1, 2)]);
}

#[test]
fn a_layer_covering_nothing_still_refuses_a_shape_the_scheduler_cannot_serve() {
    // Pruning is about what is rendered, not about what is validated: a layer
    // shape past what this scheduler serves refuses the frame wherever it was
    // recorded, so a display list that starts drawing into an empty layer does
    // not silently become servable.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(
        LayerProps {
            blend_mode: BlendMode::new(Mix::Multiply, Compose::SrcOver),
            opacity: 1.0,
            mask: None,
            clip_path: None,
        },
        None,
    );
    recorder.pop_layer();
    recorder.pop_layer();

    assert!(
        escalation(&recorder).contains("non-default blend mode"),
        "an unservable layer is refused inside an empty one too"
    );
}

// ---------------------------------------------------------------------
// A fan wider than the two groups is served by cutting the parent's round
// ---------------------------------------------------------------------

#[test]
fn a_three_wide_sibling_fan_cuts_the_root_round_and_reuses_the_first_page() {
    // GRADUATED: this shape used to escalate. Three siblings do need three
    // pages live at once *if* one pass has to composite them all — so the root
    // round is cut after the first two, both pages go back, and the third
    // sibling takes the group the first one had.
    let rounds = schedule(&sibling_fan(3));

    assert_eq!(
        rounds.len(),
        5,
        "a page each, and the surface split either side of the third: {rounds:?}"
    );
    assert_pages_survive_until_composited(&rounds);

    let pages: Vec<(u32, PageParity)> = rounds
        .iter()
        .filter_map(|round| round.page().map(|page| (page.layer, page.parity)))
        .collect();
    assert_eq!(
        pages,
        vec![
            (0, PageParity::Odd),
            (1, PageParity::Even),
            (2, PageParity::Odd)
        ],
        "the third sibling reuses the group the first one was released from"
    );

    // The cut is a split of one painter's-order walk, so the composites still
    // run in recording order — the first two in the cut round, the third after.
    assert_eq!(composite_order(&rounds), vec![(2, 0), (2, 1), (4, 2)]);
    assert!(rounds[2].is_root() && rounds[4].is_root());
    assert_eq!(rounds[2].released, vec![PageParity::Odd, PageParity::Even]);
    assert_eq!(rounds[4].released, vec![PageParity::Odd]);
    assert!(
        rounds
            .iter()
            .all(|round| !round.page().is_some_and(|page| page.continued)),
        "the surface takes the cut here, and a surface round always loads: {rounds:?}"
    );
}

#[test]
fn a_sibling_fan_of_any_width_costs_one_page_round_each_and_a_root_round_per_pair() {
    // The shape the hoisted opaque pass exists for: a staggered list entrance
    // fading five rows at once no longer skips the frame.
    for count in 1..=8_u16 {
        let rounds = schedule(&sibling_fan(count));
        assert_pages_survive_until_composited(&rounds);

        let count = usize::from(count);
        let root_rounds = rounds.iter().filter(|round| round.is_root()).count();
        assert_eq!(
            root_rounds,
            count.div_ceil(PING_PONG_GROUPS).max(1),
            "a {count}-wide fan cuts the surface once per pair of groups: {rounds:?}"
        );
        assert!(
            rounds
                .iter()
                .filter_map(Round::page)
                .all(|page| page.parity != PageParity::Spill),
            "a fan is served by cutting the surface round, never by spilling: {rounds:?}"
        );
        assert_eq!(
            rounds.iter().filter_map(Round::page).count(),
            count,
            "one page round per sibling, however wide the fan"
        );

        let composited: Vec<u32> = rounds
            .iter()
            .flat_map(Round::composites)
            .map(|c| c.layer)
            .collect();
        assert_eq!(
            composited,
            (0..count as u32).collect::<Vec<_>>(),
            "every sibling is composited, in the order the recording entered them"
        );
    }
}

#[test]
fn a_nested_sibling_pair_gives_the_parent_its_page_early_and_continues_it() {
    // GRADUATED: this shape used to escalate. A sibling pair *inside* a layer
    // costs three pages while the parent composites both in one pass — its own
    // and the two children's. Cutting the parent after the first child is what
    // keeps it to two: the parent takes its page while one group is still free,
    // composites the first child into it, and its second round loads that same
    // page rather than clearing the half already drawn.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 48, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();

    let rounds = schedule(&recorder);
    assert_pages_survive_until_composited(&rounds);

    let pages: Vec<(u32, PageParity, bool)> = rounds
        .iter()
        .filter_map(|round| {
            round
                .page()
                .map(|page| (page.layer, page.parity, page.continued))
        })
        .collect();
    assert_eq!(
        pages,
        vec![
            (1, PageParity::Even, false),
            (0, PageParity::Odd, false),
            (2, PageParity::Even, false),
            (0, PageParity::Odd, true),
        ],
        "the parent opens its page between its children and continues it after the second"
    );

    // Both children reach the parent, in recording order, and the parent
    // reaches the surface once.
    assert_eq!(composite_order(&rounds), vec![(1, 1), (3, 2), (4, 0)]);
    assert!(rounds[4].is_root());
    assert_eq!(rounds.len(), 5);
}

#[test]
fn a_flat_sibling_beside_one_that_nests_is_served_by_cutting_the_root_round() {
    // GRADUATED: this shape used to escalate. A flat sibling first, then one
    // that nests: the nesting sibling needs both groups at once, and the flat
    // one's page was still owed to the surface. Cutting the surface round pays
    // that debt early, and the nesting sibling then has both groups.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 64, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();

    let rounds = schedule(&recorder);
    assert_pages_survive_until_composited(&rounds);

    assert_eq!(rounds.len(), 5, "{rounds:?}");
    assert_eq!(
        composite_order(&rounds),
        vec![(2, 0), (3, 2), (4, 1)],
        "the flat sibling is composited in the cut round, the nesting one after it"
    );
    assert!(rounds[2].is_root() && rounds[4].is_root());
    assert_eq!(
        rounds[2].released,
        vec![PageParity::Odd],
        "the cut is what hands the flat sibling's group back"
    );
}

#[test]
fn a_cut_round_still_draws_every_range_once_and_in_recording_order() {
    // A cut splits a target's work across passes; it must not duplicate or drop
    // any of it. Draws either side of each layer, so the surface's own ranges
    // straddle both cuts.
    let mut recorder = recorder();
    for index in 0..3_u16 {
        draw(&mut recorder, index.saturating_mul(48), 0, 16);
        recorder.push_layer(layer(HALF), None);
        draw(&mut recorder, index.saturating_mul(48), 16, 32);
        recorder.pop_layer();
    }
    draw(&mut recorder, 0, 48, 16);

    let rounds = schedule(&recorder);
    assert_pages_survive_until_composited(&rounds);

    let mut issued: Vec<(u32, u32)> = rounds.iter().flat_map(draw_ranges).collect();
    issued.sort_unstable();
    assert_eq!(
        issued,
        vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 7)],
        "every recorded draw reaches exactly one round: {rounds:?}"
    );
    assert_eq!(
        rounds.iter().map(Round::draw_count).sum::<u32>(),
        recorder.draws.len() as u32
    );
}

// ---------------------------------------------------------------------
// The one spill page beside the pair
// ---------------------------------------------------------------------

#[test]
fn a_branch_whose_second_subtree_nests_is_served_on_the_spill_page() {
    // GRADUATED: this shape used to escalate, and it is the shape a navigation
    // transition holds for the whole of a scrub — a full-screen page layer
    // holding a chip beside a chip that carries a translucent chip of its own.
    // The parent takes a group at the cut its first child forces and keeps it,
    // the grandchild takes the other, and the child hosting the grandchild has
    // to sample that page while rendering its own: three pages really are live
    // there, and the spill page is the third.
    let rounds = schedule(&branching_chain());
    assert_pages_survive_until_composited(&rounds);

    let pages: Vec<(u32, usize, PageParity, bool)> = rounds
        .iter()
        .filter_map(|round| {
            round
                .page()
                .map(|page| (page.layer, page.depth, page.parity, page.continued))
        })
        .collect();
    assert_eq!(
        pages,
        vec![
            (1, 2, PageParity::Even, false),
            (0, 1, PageParity::Odd, false),
            (3, 3, PageParity::Even, false),
            (2, 2, PageParity::Spill, false),
            (0, 1, PageParity::Odd, true),
        ],
        "the parent opens its page at the cut and continues it after the spilled child: {rounds:?}"
    );
    assert_eq!(
        rounds.len(),
        6,
        "five page rounds and the surface: {rounds:?}"
    );

    // Every layer reaches its parent exactly once, in recording order, and the
    // spill page goes back to the pool in the very round that samples it.
    assert_eq!(
        composite_order(&rounds),
        vec![(1, 1), (3, 3), (4, 2), (5, 0)]
    );
    assert_eq!(rounds[4].released, vec![PageParity::Spill]);
    assert_eq!(
        rounds
            .iter()
            .filter(|round| round
                .page()
                .is_some_and(|page| page.parity == PageParity::Spill))
            .count(),
        1,
        "one layer needed the third page, so one round writes it: {rounds:?}"
    );
}

#[test]
fn the_same_branch_recorded_at_the_root_still_needs_no_spill_page() {
    // The contrast that makes the case above a transition defect rather than a
    // content one: the identical children recorded at the root — the screen at
    // rest, before a transition wraps it in a page layer — have always been
    // served by cutting the surface round, because no ancestor is holding a
    // page across the walk. Nothing about that changes.
    let rounds = schedule(&branching_chain_at_the_root());
    assert_pages_survive_until_composited(&rounds);

    assert!(
        rounds
            .iter()
            .filter_map(Round::page)
            .all(|page| page.parity != PageParity::Spill),
        "the root-level shape is served on the pair alone: {rounds:?}"
    );
    let composited: Vec<u32> = rounds
        .iter()
        .flat_map(Round::composites)
        .map(|c| c.layer)
        .collect();
    assert_eq!(
        composited,
        vec![0, 2, 1],
        "every layer is composited once, innermost before the layer that holds it"
    );
}

#[test]
fn a_fan_of_nesting_siblings_reuses_the_one_spill_page_rather_than_stacking_them() {
    // The spill is one page, and this is what that costs and buys: a parent
    // holding a flat child and then three nesting ones needs the third page
    // once per nesting child, and gets it every time — because the previous
    // spilled child's composite is sitting in the parent's open round, which
    // `make_room`'s cut hands back before the next one asks.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 16, 32);
    recorder.pop_layer();
    for index in 1..4_u16 {
        recorder.push_layer(layer(HALF), None);
        recorder.push_layer(layer(HALF), None);
        draw(&mut recorder, index.saturating_mul(48), 16, 32);
        recorder.pop_layer();
        recorder.pop_layer();
    }
    recorder.pop_layer();

    let rounds = schedule(&recorder);
    assert_pages_survive_until_composited(&rounds);

    let spilled = rounds
        .iter()
        .filter(|round| {
            round
                .page()
                .is_some_and(|page| page.parity == PageParity::Spill)
        })
        .count();
    assert_eq!(
        spilled, 3,
        "each nesting sibling takes the one spill page in turn: {rounds:?}"
    );
    for round in &rounds {
        let live = usize::from(round.page().is_some()) + round.released.len();
        assert!(
            live <= MAX_LIVE_PAGES,
            "no round holds more than the bound live: {round:?}"
        );
    }

    // Every layer still reaches exactly one composite, and the recording's own
    // order survives being cut across that many rounds.
    let composited: Vec<u32> = rounds
        .iter()
        .flat_map(Round::composites)
        .map(|c| c.layer)
        .collect();
    assert_eq!(composited, vec![1, 3, 2, 5, 4, 7, 6, 0]);
    assert_eq!(
        composited.len(),
        recorder.layers.len(),
        "no layer is composited twice and none is dropped"
    );
}

// ---------------------------------------------------------------------
// Shapes the scheduler refuses
// ---------------------------------------------------------------------

#[test]
fn a_sibling_pair_nested_inside_a_sibling_pair_escalates() {
    // A fan inside a layer costs that layer its group for the rest of the
    // frame, and a fan two levels deep wants the same of its own parent. The
    // spill page carries the inner fan's second child — but the inner parent's
    // own round then has to composite *both* of its children, so it needs a
    // page while three are already live: its parent's, and one per child. A
    // cut cannot pay for this one either, so the frame is refused rather than
    // rendered wrong.
    let reason = escalation(&nested_fan_inside_a_fan());

    assert!(
        reason.contains("layer 2"),
        "the reason names the layer that found no page: {reason}"
    );
    assert!(
        reason.contains("fourth live intermediate page"),
        "the reason says what ran out: {reason}"
    );
    assert!(
        reason.contains("spill page"),
        "the reason says the spill page was tried too: {reason}"
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
fn a_layer_taller_than_the_ceiling_is_refused_rather_than_clipped_to_it() {
    // Bands are columns (E14): they split width, never height, so a layer
    // taller than the ceiling is still refused exactly as it always was,
    // whatever its width. Two draws far apart in `y`, so the layer's bbox
    // spans well past a 64-texel ceiling even though each draw is only one
    // tile row tall.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 0, 16);
    draw(&mut recorder, 0, 64, 16);
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
        "a layer taller than the ceiling is refused, not shrunk onto it"
    );

    // The same recording schedules under the default bounds, so the refusal is
    // the ceiling's doing and not the recording's.
    assert_eq!(schedule(&recorder).len(), 2);
}

// ---------------------------------------------------------------------
// A layer wider than the ceiling is banded into column pages (E14)
// ---------------------------------------------------------------------

#[test]
fn a_layer_wider_than_the_ceiling_is_banded_into_column_pages_rather_than_refused() {
    // GRADUATED: this shape used to escalate. A layer wider than any single
    // page splits into full-height column bands under a 64-texel ceiling:
    // 200 needs `div_ceil(200, 64) == 4` of them.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 0, 200);
    recorder.pop_layer();

    let config = PageConfig {
        min_page_size: 64,
        max_page_size: 64,
    };
    let rounds = Schedule::build(&recorder, &caps(), &config).expect("a wide layer bands");
    assert_pages_survive_until_composited(&rounds);

    let bands: Vec<RectU16> = rounds
        .iter()
        .filter_map(|round| round.page().map(|page| page.bounds))
        .collect();
    assert_eq!(
        bands.len(),
        4,
        "one page round per band, however the surface's own rounds are split: {rounds:?}"
    );

    // The bands tile the layer's own bounds exactly: they abut, none
    // overlaps, and every one spans the same (one-tile-row) height.
    let mut x = 0_u16;
    for bounds in &bands {
        assert_eq!(bounds.x0, x, "bands abut with no gap or overlap");
        assert_eq!(bounds.y0, 0);
        assert_eq!(bounds.y1, 4, "every band spans the layer's own height");
        x = bounds.x1;
    }
    assert_eq!(x, 200, "the bands cover the layer's width exactly");

    // Every page round replays the very same draw the layer recorded: a band
    // is a difference in which page and rectangle the content lands at, never
    // in the content itself.
    for round in rounds.iter().filter(|round| round.page().is_some()) {
        assert_eq!(draw_ranges(round), vec![(0, 1)]);
    }

    // Every band is composited at its own rectangle, in band order, each
    // still carrying the layer's own opacity.
    let composited: Vec<(RectU16, f32)> = rounds
        .iter()
        .flat_map(Round::composites)
        .map(|c| {
            assert_eq!(c.layer, 0);
            (c.bounds, c.opacity)
        })
        .collect();
    assert_eq!(
        composited,
        bands
            .iter()
            .map(|bounds| (*bounds, HALF))
            .collect::<Vec<_>>(),
        "the composites land in band order, at each band's own rectangle: {rounds:?}"
    );
}

#[test]
fn a_width_needing_more_bands_than_the_scheduler_allows_is_still_refused() {
    let config = PageConfig {
        min_page_size: 64,
        max_page_size: 64,
    };
    let ceiling = pages::page_ceiling(&config, &caps());
    // One tile-width past the largest width the band bound serves: a strip's
    // own width has to be tile-aligned, so `+1` is not a legal draw here.
    let width = ceiling * pages::MAX_PAGE_BANDS as u32 + u32::from(Tile::WIDTH);

    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(
        &mut recorder,
        0,
        0,
        u16::try_from(width).expect("stays inside the device grid in this test"),
    );
    recorder.pop_layer();

    assert!(
        matches!(
            Schedule::build(&recorder, &caps(), &config),
            Err(EngineError::IntermediateTextureTooLarge)
        ),
        "a width past the band bound is refused rather than split further"
    );
}

#[test]
fn a_wide_layer_holding_a_nested_isolated_child_is_still_refused_rather_than_banded() {
    // A band's ops are replayed once per band, and replaying a nested
    // child's own composite would read a page a later band has already
    // reused — banding is scoped to a layer with no isolated child of its
    // own, and a wide layer that has one is refused exactly as before.
    let mut recorder = recorder();
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 0, 0, 200);
    recorder.push_layer(layer(HALF), None);
    draw(&mut recorder, 16, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();

    let config = PageConfig {
        min_page_size: 64,
        max_page_size: 64,
    };
    assert!(
        matches!(
            Schedule::build(&recorder, &caps(), &config),
            Err(EngineError::IntermediateTextureTooLarge)
        ),
        "a wide layer with a nested child is refused rather than banded"
    );
}

#[test]
fn a_refused_frame_never_reports_a_target_it_would_have_rendered() {
    // Every escalation path returns the error rather than a partial schedule:
    // a caller that took a `Vec<Round>` from a refused frame would render a
    // layer graph it had already been told the scheduler cannot serve. The
    // second recorder is the one that matters here — its escalation happens
    // after several rounds have already been pushed, cut rounds and a spill
    // page among them.
    let recorders = [nested_chain(MAX_CHAIN_DEPTH + 1), nested_fan_inside_a_fan()];

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
