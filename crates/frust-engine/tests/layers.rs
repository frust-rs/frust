//! Group lowering: what an opacity layer, a retained bracket and a hole punch
//! cost, and what each one actually leaves painted.
//!
//! Every case drives the public seam — a `frust_scene::Scene` recorded through
//! `SceneBuilder`, compiled by [`SceneCompiler`] — and reads the result back
//! through the same pairwise strip walk the renderer uses, so the assertions
//! pin the pixels a GPU pass would produce rather than the compiler's
//! internals. No GPU, device or surface is involved; the scheduler is consulted
//! for the pass count a recorded layer costs, which is a decision over the
//! recording's shape and needs no device either.
//!
//! The claims under test are the ones the lowering exists for: a layer at full
//! opacity is a clip and nothing more, a translucent one is a page of its own,
//! a retained bracket paints its body exactly as the equivalent
//! transform-and-layer pair would, and a clear is hoisted out of every bracket
//! around it, lowered to coverage that is exact at a tile-unaligned edge, and
//! carries a depth that orders it against opaque coverage and nothing else.
//!
//! Pixel comparison against the CPU reference renderer is a separate, GPU-bound
//! concern and lives with the phase's golden corpus, not here.

use core::ops::Range;

use frust_engine::compile::CompiledFrame;
use frust_engine::schedule::{PageConfig, RetainedLayers, Round, RoundOp, RoundTarget, Schedule};
use frust_engine::{EngineError, RetainedKind, SceneCompiler};
use frust_gpu::{DownlevelProfile, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, BezPath, Rect};
use peniko::Brush;
use peniko::color::palette::css::RED;
use vello_common::tile::Tile;
use wgpu::naga;

/// Viewport every case compiles against. Deliberately not square, so an axis
/// swapped somewhere in the lowering cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (64, 48);

/// The opacity that makes a layer isolating — strictly between transparent and
/// opaque.
const HALF: f32 = 0.5;

/// A rectangle whose every edge falls on a whole pixel but on no tile boundary
/// — the punch geometry the destination-out lowering has to keep exact.
const TILE_UNALIGNED: Rect = Rect::new(6.0, 6.0, 18.0, 14.0);

fn red() -> Brush {
    Brush::Solid(RED)
}

/// A scene built by `record`, ready to compile.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

fn compile(scene: &Scene) -> CompiledFrame {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

fn refusal(scene: &Scene) -> EngineError {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect_err("a scene carrying non-finite geometry is refused")
}

/// The rounds the frame's recording schedules as.
fn rounds(frame: &CompiledFrame) -> Vec<Round> {
    Schedule::build(
        &frame.recorder,
        &TierCaps::fake(DownlevelProfile::Full),
        &PageConfig::default(),
    )
    .expect("a chain of opacity layers schedules")
}

/// The opacity each round's page composites at, innermost first.
fn page_opacities(rounds: &[Round]) -> Vec<f32> {
    rounds
        .iter()
        .flat_map(|round| round.composites())
        .map(|composite| composite.opacity)
        .collect()
}

/// The strip ranges `frame`'s draws cover, in paint order.
fn draw_ranges(frame: &CompiledFrame) -> Vec<Range<usize>> {
    frame
        .draws()
        .iter()
        .map(|draw| draw.strip_range.clone())
        .collect()
}

/// The strip ranges `frame`'s hole punches cover.
fn punch_ranges(frame: &CompiledFrame) -> Vec<Range<usize>> {
    frame
        .clears
        .iter()
        .map(|punch| punch.strip_range.clone())
        .collect()
}

/// `ranges` of the frame's strip storage rasterized back into a coverage grid,
/// one byte per pixel.
///
/// Reads a strip run exactly as `EngineRenderer`'s instance builder does — a
/// strip's own alpha-sampled span, plus the solid span filling the gap to the
/// next strip when the winding between them says there is one — so a lowering
/// that produced a run the renderer would read differently shows up here as
/// wrong pixels rather than passing on a structural technicality.
fn coverage_of(frame: &CompiledFrame, ranges: impl IntoIterator<Item = Range<usize>>) -> Vec<u8> {
    let (width, height) = (usize::from(VIEWPORT.0), usize::from(VIEWPORT.1));
    let mut grid = vec![0_u8; width * height];
    let mut paint = |x: usize, y: usize, value: u8| {
        if x < width
            && y < height
            && let Some(pixel) = grid.get_mut(y * width + x)
        {
            *pixel = (*pixel).max(value);
        }
    };

    let strips = frame.strip_buf();
    let alphas = frame.alphas();
    for range in ranges {
        let run = strips
            .get(range)
            .expect("a recorded range is inside the frame's strip buffer");
        for pair in run.windows(2) {
            let (strip, next) = (pair[0], pair[1]);
            assert!(!strip.is_sentinel(), "a sentinel is never a pair's first");

            let span = strip.width_to(&next);
            for column in 0..usize::from(span) {
                for row in 0..usize::from(Tile::HEIGHT) {
                    let index =
                        strip.alpha_idx() as usize + column * usize::from(Tile::HEIGHT) + row;
                    let value = alphas.get(index).copied().unwrap_or(0);
                    paint(
                        usize::from(strip.x) + column,
                        usize::from(strip.y) + row,
                        value,
                    );
                }
            }

            if next.fill_gap() && next.y == strip.y {
                let gap_x = strip.x.saturating_add(span);
                for x in gap_x..next.x {
                    for row in 0..usize::from(Tile::HEIGHT) {
                        paint(usize::from(x), usize::from(strip.y) + row, 255);
                    }
                }
            }
        }
    }
    grid
}

/// Everything the frame's draws paint.
fn coverage(frame: &CompiledFrame) -> Vec<u8> {
    coverage_of(frame, draw_ranges(frame))
}

/// Everything the frame's hole punches erase.
fn punch_coverage(frame: &CompiledFrame) -> Vec<u8> {
    coverage_of(frame, punch_ranges(frame))
}

/// The bounding box of every covered pixel in `grid`, or `None` when nothing is
/// covered.
fn covered_bbox(grid: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let width = usize::from(VIEWPORT.0);
    let mut bbox: Option<(usize, usize, usize, usize)> = None;
    for (index, value) in grid.iter().enumerate() {
        if *value == 0 {
            continue;
        }
        let (x, y) = (index % width, index / width);
        bbox = Some(match bbox {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }
    bbox
}

/// Whether the pixel at `(x, y)` is inside `rect`.
fn inside(rect: Rect, x: usize, y: usize) -> bool {
    let (x, y) = (x as f64, y as f64);
    x >= rect.x0 && x < rect.x1 && y >= rect.y0 && y < rect.y1
}

// ---------------------------------------------------------------------
// Opacity layers
// ---------------------------------------------------------------------

#[test]
fn a_layer_at_full_opacity_is_its_clip_and_nothing_more() {
    let body = Rect::new(0.0, 0.0, 64.0, 48.0);
    let group = Rect::new(8.0, 8.0, 40.0, 32.0);

    let layered = compile(&scene_of(|builder| {
        builder.push_layer(group, 1.0);
        builder.fill_rect(body, red());
        builder.pop_layer();
    }));
    let clipped = compile(&scene_of(|builder| {
        builder.push_clip(group);
        builder.fill_rect(body, red());
        builder.pop_clip();
    }));

    assert!(
        layered.recorder.layers.is_empty(),
        "a full-opacity layer needs no page and records no layer"
    );
    assert_eq!(layered.draws().len(), 1);
    assert_eq!(coverage(&layered), coverage(&clipped));
    assert_eq!(rounds(&layered).len(), 1, "one pass, the surface's own");
}

#[test]
fn a_translucent_layer_is_clipped_by_its_own_rectangle() {
    let body = Rect::new(0.0, 0.0, 64.0, 48.0);
    let group = Rect::new(8.0, 8.0, 40.0, 32.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_layer(group, HALF);
        builder.fill_rect(body, red());
        builder.pop_layer();
    }));

    let painted = coverage(&frame);
    for y in 0..usize::from(VIEWPORT.1) {
        for x in 0..usize::from(VIEWPORT.0) {
            let value = painted[y * usize::from(VIEWPORT.0) + x];
            if inside(group, x, y) {
                assert_eq!(value, 255, "({x}, {y}) is inside the layer's rectangle");
            } else {
                assert_eq!(value, 0, "({x}, {y}) is outside the layer's rectangle");
            }
        }
    }
}

#[test]
fn three_nested_translucent_layers_take_a_page_each() {
    let frame = compile(&scene_of(|builder| {
        builder.push_layer(Rect::new(0.0, 0.0, 48.0, 40.0), 0.25);
        builder.fill_rect(Rect::new(0.0, 0.0, 16.0, 16.0), red());
        builder.push_layer(Rect::new(4.0, 4.0, 40.0, 36.0), HALF);
        builder.fill_rect(Rect::new(8.0, 8.0, 24.0, 24.0), red());
        builder.push_layer(Rect::new(8.0, 8.0, 32.0, 32.0), 0.75);
        builder.fill_rect(Rect::new(12.0, 12.0, 28.0, 28.0), red());
        builder.pop_layer();
        builder.pop_layer();
        builder.pop_layer();
    }));

    assert_eq!(frame.recorder.layers.len(), 3);
    assert_eq!(frame.draws().len(), 3);

    let rounds = rounds(&frame);
    assert_eq!(rounds.len(), 4, "three pages plus the surface");
    assert!(
        rounds
            .iter()
            .take(3)
            .all(|round| matches!(round.target, RoundTarget::Page(_))),
        "the innermost layers are rendered first, each into a page"
    );
    assert!(rounds[3].is_root());
    // Innermost first: the deepest layer is scheduled before its parent.
    assert_eq!(page_opacities(&rounds), vec![0.75, HALF, 0.25]);
}

#[test]
fn an_unbalanced_pop_layer_lifts_only_the_innermost_open_bracket() {
    let body = Rect::new(0.0, 0.0, 64.0, 48.0);
    let outer = Rect::new(8.0, 8.0, 40.0, 32.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_layer(outer, 1.0);
        builder.pop_layer();
        // Nothing is open now; a second pop must not lift the clip a later
        // sibling relies on, because there is nothing left to lift.
        builder.pop_layer();
        builder.push_clip(Rect::new(0.0, 0.0, 16.0, 16.0));
        builder.fill_rect(body, red());
    }));

    assert_eq!(
        covered_bbox(&coverage(&frame)),
        Some((0, 0, 16, 16)),
        "the clip pushed after the stray pop is still in force"
    );
}

#[test]
fn a_layer_left_open_is_closed_at_the_end_of_the_frame() {
    let frame = compile(&scene_of(|builder| {
        builder.push_layer(Rect::new(4.0, 4.0, 40.0, 40.0), HALF);
        builder.fill_rect(Rect::new(8.0, 8.0, 24.0, 24.0), red());
    }));

    assert_eq!(frame.recorder.layers.len(), 1);
    assert!(
        !frame.recorder.has_layers(),
        "an unclosed layer is closed by the compiler, so the recording is whole"
    );
    assert!(
        !frame.recorder.layers[0].bbox.is_empty(),
        "a layer closed at the end of the frame still has the bounds its pop computes"
    );
    assert_eq!(rounds(&frame).len(), 2, "the page plus the surface");
}

// ---------------------------------------------------------------------
// Retained brackets: the inline lowering
// ---------------------------------------------------------------------

#[test]
fn a_retained_body_lowers_like_the_equivalent_transform_and_layer_pair() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);
    let body = Rect::new(12.0, 12.0, 36.0, 36.0);

    let bracketed = compile(&scene_of(|builder| {
        builder.push_retained(7, rect, HALF, 0.5);
        builder.fill_rect(body, red());
        builder.pop_retained();
    }));
    let equivalent = compile(&scene_of(|builder| {
        builder.push_transform(Affine::scale_about(0.5, rect.center()));
        builder.push_layer(rect, HALF);
        builder.fill_rect(body, red());
        builder.pop_layer();
        builder.pop_transform();
    }));

    assert_eq!(coverage(&bracketed), coverage(&equivalent));
    assert_eq!(
        page_opacities(&rounds(&bracketed)),
        page_opacities(&rounds(&equivalent))
    );
}

#[test]
fn a_retained_bracket_scales_its_body_about_the_rectangle_centre() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_retained(1, rect, 1.0, 0.5);
        builder.fill_rect(rect, red());
        builder.pop_retained();
    }));

    // Halved about (24, 24): every edge moves halfway to the centre.
    assert_eq!(covered_bbox(&coverage(&frame)), Some((16, 16, 32, 32)));
    assert!(
        frame.recorder.layers.is_empty(),
        "a bracket at full opacity needs no page"
    );
}

#[test]
fn a_retained_bracket_scales_and_fades_together() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_retained(2, rect, 0.25, 0.5);
        builder.fill_rect(rect, red());
        builder.pop_retained();
    }));

    assert_eq!(covered_bbox(&coverage(&frame)), Some((16, 16, 32, 32)));
    assert_eq!(page_opacities(&rounds(&frame)), vec![0.25]);
}

#[test]
fn only_the_outermost_retained_bracket_is_honoured() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);
    let inner = Rect::new(12.0, 12.0, 36.0, 36.0);

    let nested = compile(&scene_of(|builder| {
        builder.push_retained(1, rect, HALF, 0.5);
        builder.push_retained(2, inner, 0.25, 4.0);
        builder.fill_rect(rect, red());
        builder.pop_retained();
        builder.pop_retained();
    }));
    let outer_only = compile(&scene_of(|builder| {
        builder.push_retained(1, rect, HALF, 0.5);
        builder.fill_rect(rect, red());
        builder.pop_retained();
    }));

    assert_eq!(coverage(&nested), coverage(&outer_only));
    assert_eq!(nested.recorder.layers.len(), 1, "one bracket, one layer");
    assert_eq!(
        page_opacities(&rounds(&nested)),
        page_opacities(&rounds(&outer_only))
    );
}

#[test]
fn an_unbalanced_pop_retained_is_ignored() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.pop_retained();
        builder.push_retained(1, rect, 1.0, 0.5);
        builder.pop_retained();
        builder.pop_retained();
        // The stray pops must leave no correction behind, so this paints where
        // it was recorded rather than scaled.
        builder.fill_rect(rect, red());
    }));

    assert_eq!(covered_bbox(&coverage(&frame)), Some((8, 8, 40, 40)));
    assert!(frame.recorder.layers.is_empty());
}

#[test]
fn a_retained_bracket_left_open_stops_correcting_at_the_end_of_the_frame() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let first = compile(&scene_of(|builder| {
        builder.push_retained(1, rect, HALF, 0.5);
        builder.fill_rect(rect, red());
    }));
    // The compiler holds no per-frame state between calls, so a second frame
    // through the same compiler must not inherit the first's correction.
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    let open = scene_of(|builder| {
        builder.push_retained(1, rect, HALF, 0.5);
        builder.fill_rect(rect, red());
    });
    let plain = scene_of(|builder| builder.fill_rect(rect, red()));
    compiler
        .compile(&open, Affine::IDENTITY, VIEWPORT)
        .expect("an unclosed bracket compiles");
    let second = compiler
        .compile(&plain, Affine::IDENTITY, VIEWPORT)
        .expect("the next frame compiles");

    assert_eq!(covered_bbox(&coverage(&first)), Some((16, 16, 32, 32)));
    assert_eq!(covered_bbox(&coverage(&second)), Some((8, 8, 40, 40)));
}

// ---------------------------------------------------------------------
// The hole punch
// ---------------------------------------------------------------------

#[test]
fn a_clear_is_lowered_to_coverage_that_paints_no_draw() {
    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 32.0, 32.0), red());
        builder.clear_rect(TILE_UNALIGNED);
    }));

    assert_eq!(frame.clears.len(), 1);
    assert_eq!(
        frame.draws().len(),
        1,
        "the punch erases rather than paints, so it records no draw"
    );
    // Dropping the punch pass — what a target that disregards alpha does — has
    // to leave the frame exactly as it would have been without the clear.
    let without = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 32.0, 32.0), red());
    }));
    assert_eq!(coverage(&frame), coverage(&without));
}

#[test]
fn a_punch_is_exact_at_a_tile_unaligned_edge() {
    let frame = compile(&scene_of(|builder| {
        builder.clear_rect(TILE_UNALIGNED);
    }));

    assert_eq!(frame.clears.len(), 1);
    let erased = punch_coverage(&frame);
    for y in 0..usize::from(VIEWPORT.1) {
        for x in 0..usize::from(VIEWPORT.0) {
            let value = erased[y * usize::from(VIEWPORT.0) + x];
            let expected = if inside(TILE_UNALIGNED, x, y) { 255 } else { 0 };
            assert_eq!(
                value, expected,
                "({x}, {y}) must be fully erased or untouched, never feathered"
            );
        }
    }
}

#[test]
fn a_punch_inside_a_layer_is_hoisted_out_of_it_and_confined_to_it() {
    let group = Rect::new(8.0, 8.0, 24.0, 24.0);
    let hole = Rect::new(16.0, 16.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_layer(group, HALF);
        builder.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), red());
        builder.clear_rect(hole);
        builder.pop_layer();
    }));

    assert_eq!(frame.clears.len(), 1);
    assert_eq!(
        covered_bbox(&punch_coverage(&frame)),
        Some((16, 16, 24, 24)),
        "the punch keeps the region its bracket admitted it to, and nothing more"
    );

    let rounds = rounds(&frame);
    assert_eq!(rounds.len(), 2, "the layer's page plus the surface");
    assert!(
        frame.clears[0].depth < u32::MAX,
        "the punch carries a painter-order depth of its own"
    );
}

#[test]
fn a_punch_no_bracket_admits_is_dropped() {
    let frame = compile(&scene_of(|builder| {
        builder.push_clip(Rect::new(0.0, 0.0, 8.0, 8.0));
        builder.clear_rect(Rect::new(32.0, 32.0, 40.0, 40.0));
        builder.pop_clip();
    }));

    assert!(frame.clears.is_empty());
}

#[test]
fn a_punch_sits_behind_every_draw_recorded_after_it() {
    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 32.0, 32.0), red());
        builder.clear_rect(TILE_UNALIGNED);
        builder.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), red());
    }));

    let punch = frame.clears[0].depth;
    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    assert_eq!(depths, vec![0, 2], "the punch takes a depth of its own");
    assert_eq!(punch, 1);
    assert!(
        depths[0] < punch && punch < depths[1],
        "the punch sits behind what was painted before it and in front of nothing"
    );
}

#[test]
fn a_translucent_composite_recorded_after_a_clear_is_not_ordered_ahead_of_the_punch() {
    // The shape the narrowed ordering contract is about: an opaque backdrop, a
    // hole punched through it, and a translucent layer recorded over the hole
    // afterwards. Painter order puts the layer in front of the punch, and the
    // depth data below says so — but a composite writes no depth, so the
    // trailing punch pass is not held off it the way opaque chrome would be.
    let backdrop = Rect::new(0.0, 0.0, 64.0, 48.0);
    let fading = Rect::new(8.0, 8.0, 24.0, 20.0);

    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(backdrop, red());
        builder.clear_rect(TILE_UNALIGNED);
        builder.push_layer(fading, HALF);
        builder.fill_rect(fading, red());
        builder.pop_layer();
    }));

    assert_eq!(frame.clears.len(), 1);
    let punch = &frame.clears[0];

    // The recorded ordering: backdrop, punch, then the layer's own contents.
    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    assert_eq!(depths.len(), 2, "the punch paints nothing of its own");
    assert!(
        depths[0] < punch.depth && punch.depth < depths[1],
        "the punch takes a depth between the backdrop and the fading layer: {depths:?}"
    );

    // The layer reaches the surface as a composite in the root round, and the
    // punch pass runs after every round — so the depth attachment is all that
    // could order the two, and a composite never writes it.
    let rounds = rounds(&frame);
    let root = rounds
        .last()
        .expect("a schedule always ends at the surface");
    assert!(root.is_root());
    assert_eq!(
        root.composites().count(),
        1,
        "the fading layer arrives at the surface as a composite: {rounds:?}"
    );

    // The punch's coverage is generated past every draw's, which is what puts
    // its pass last in the frame.
    let last_draw_end = draw_ranges(&frame)
        .iter()
        .map(|range| range.end)
        .max()
        .unwrap_or(0);
    assert!(
        punch.strip_range.start >= last_draw_end,
        "the punch's strips sit past every draw's: {:?} against {last_draw_end}",
        punch.strip_range
    );

    // And the two genuinely overlap, so none of the above is vacuous.
    let composite = root
        .composites()
        .next()
        .expect("the root round composites the layer");
    let bounds = composite.bounds;
    assert!(
        f64::from(bounds.x0) < TILE_UNALIGNED.x1
            && TILE_UNALIGNED.x0 < f64::from(bounds.x1)
            && f64::from(bounds.y0) < TILE_UNALIGNED.y1
            && TILE_UNALIGNED.y0 < f64::from(bounds.y1),
        "the punch and the composite cover common pixels: {bounds:?}"
    );
}

#[test]
fn opaque_coverage_recorded_after_a_clear_is_the_only_paint_the_depth_test_saves() {
    // The other half of the same contract, and the reason the depth is carried
    // at all: coverage recorded after the clear takes a strictly nearer depth,
    // which is what the punch pass's `LessEqual` test rejects it on. Everything
    // the punch is ordered against has to reach the depth-writing pass, so this
    // pins the ordering data rather than the blend state (the GPU-side pin
    // lives with the encode contract).
    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 32.0, 32.0), red());
        builder.clear_rect(TILE_UNALIGNED);
        builder.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), red());
        builder.clear_rect(Rect::new(20.0, 20.0, 28.0, 28.0));
    }));

    assert_eq!(frame.clears.len(), 2);
    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    let punches: Vec<u32> = frame.clears.iter().map(|punch| punch.depth).collect();

    assert_eq!(
        depths,
        vec![0, 2],
        "each clear consumes a depth of its own from the same painter-order run"
    );
    assert_eq!(punches, vec![1, 3]);
    assert!(
        punches.windows(2).all(|pair| pair[0] < pair[1]),
        "two punches keep their recorded order: {punches:?}"
    );
    assert!(
        punches[1] > depths[1],
        "the second punch is recorded after every draw, so nothing in the frame's own paint \
         survives its depth test: {punches:?} against {depths:?}"
    );
}

#[test]
fn two_clears_lower_to_two_punches() {
    let frame = compile(&scene_of(|builder| {
        builder.clear_rect(Rect::new(4.0, 4.0, 12.0, 12.0));
        builder.clear_rect(Rect::new(20.0, 20.0, 28.0, 28.0));
    }));

    assert_eq!(frame.clears.len(), 2);
    assert_ne!(frame.clears[0].strip_range, frame.clears[1].strip_range);
    assert!(frame.clears[0].depth < frame.clears[1].depth);
}

// ---------------------------------------------------------------------
// Totality
// ---------------------------------------------------------------------

#[test]
fn non_finite_group_geometry_is_refused_before_any_strip_is_generated() {
    let cases: Vec<(&str, Scene)> = vec![
        (
            "a layer rectangle",
            scene_of(|builder| {
                builder.push_layer(Rect::new(f64::NAN, 0.0, 16.0, 16.0), HALF);
                builder.pop_layer();
            }),
        ),
        (
            "a layer opacity",
            scene_of(|builder| {
                builder.push_layer(Rect::new(0.0, 0.0, 16.0, 16.0), f32::NAN);
                builder.pop_layer();
            }),
        ),
        (
            "a clear rectangle",
            scene_of(|builder| builder.clear_rect(Rect::new(0.0, 0.0, f64::INFINITY, 16.0))),
        ),
        (
            "a retained bracket scale",
            scene_of(|builder| {
                builder.push_retained(1, Rect::new(0.0, 0.0, 16.0, 16.0), 1.0, f64::NAN);
                builder.pop_retained();
            }),
        ),
    ];

    for (what, scene) in cases {
        assert!(
            matches!(refusal(&scene), EngineError::InvalidGeometry),
            "{what} that is not finite must refuse the frame"
        );
    }
}

#[test]
fn a_singular_retained_transform_drops_the_scale_rather_than_the_frame() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        // A collapsed transform has no usable inverse, so the presentation
        // scale cannot be conjugated through it.
        builder.push_transform(Affine::scale(0.0));
        builder.push_retained(1, rect, 1.0, 0.5);
        builder.pop_retained();
        builder.pop_transform();
        builder.fill_rect(rect, red());
    }));

    assert_eq!(
        covered_bbox(&coverage(&frame)),
        Some((8, 8, 40, 40)),
        "the body draws unscaled rather than under a non-finite correction"
    );
}

#[test]
fn a_command_a_correction_pushes_off_the_grid_is_skipped_not_refused() {
    let rect = Rect::new(0.0, 0.0, 16.0, 16.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_retained(1, rect, 1.0, 1e200);
        // On its own this transform is finite; composed with the bracket's
        // correction it is not, and no `u16` pixel could hold what it maps to.
        builder.push_transform(Affine::scale(1e200));
        builder.fill_rect(rect, red());
        builder.pop_transform();
        builder.pop_retained();
        builder.fill_rect(Rect::new(4.0, 4.0, 12.0, 12.0), red());
    }));

    assert_eq!(
        covered_bbox(&coverage(&frame)),
        Some((4, 4, 12, 12)),
        "the skipped command draws nothing and the rest of the frame is intact"
    );
}

#[test]
fn a_bracket_a_correction_pushes_off_the_grid_still_balances_its_own_pop() {
    let rect = Rect::new(0.0, 0.0, 16.0, 16.0);
    let outside = Rect::new(20.0, 20.0, 36.0, 36.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_retained(1, rect, 1.0, 1e200);
        builder.push_transform(Affine::scale(1e200));
        // Composed with the bracket's correction this clip lands nowhere a
        // `u16` pixel could hold, so it admits nothing at all.
        builder.push_clip(rect);
        builder.fill_rect(rect, red());
        builder.pop_clip();
        builder.pop_transform();
        builder.pop_retained();
        builder.fill_rect(outside, red());
    }));

    assert_eq!(
        covered_bbox(&coverage(&frame)),
        Some((20, 20, 36, 36)),
        "the stray clip's pop closed the bracket that clip opened, not the frame"
    );
}

// ---------------------------------------------------------------------
// A retained body under a path clip
// ---------------------------------------------------------------------

/// The body every retained-bracket case below records, in its own space.
const RETAINED_BODY: Rect = Rect::new(0.0, 0.0, 16.0, 16.0);

/// A star-ish outline covering the whole viewport's middle — wide enough that
/// a body inside it is never cut by the outline's own edge, so the only thing
/// that can make it unretainable is that the clip is a mask.
fn wide_outline() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((32.0, -40.0));
    path.line_to((110.0, 24.0));
    path.line_to((32.0, 90.0));
    path.line_to((-46.0, 24.0));
    path.close_path();
    path
}

/// Whether each `PushRetained` the frame recorded may be held by a page.
fn retainable(frame: &CompiledFrame) -> Vec<bool> {
    frame
        .retained
        .iter()
        .filter_map(|bracket| match bracket.kind {
            RetainedKind::Record { retainable, .. } => Some(retainable),
            RetainedKind::Draw { .. } => None,
        })
        .collect()
}

/// `scene` compiled with retention on — the only compiler that records a
/// `RetainedBracket` at all.
fn compile_retained(scene: &Scene) -> CompiledFrame {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_retained_layers(true);
    compiler
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// A solid [`RETAINED_BODY`] in a `PushRetained` bracket at `(8, 8)`.
fn record_retained_body(builder: &mut SceneBuilder<'_>) {
    builder.push_transform(Affine::translate((8.0, 8.0)));
    builder.push_retained(1, RETAINED_BODY, 1.0, 1.0);
    builder.fill_rect(RETAINED_BODY, red());
    builder.pop_retained();
    builder.pop_transform();
}

#[test]
fn a_retained_body_under_a_path_clip_is_marked_unretainable() {
    let plain = compile_retained(&scene_of(record_retained_body));
    assert_eq!(
        retainable(&plain),
        vec![true],
        "the control: the same body under no clip is retainable"
    );

    let under_path = compile_retained(&scene_of(|builder| {
        builder.push_clip_path(wide_outline());
        record_retained_body(builder);
        builder.pop_clip();
    }));
    assert_eq!(under_path.mask_clips, 1, "the path clip lowered to a mask");
    assert_eq!(
        retainable(&under_path),
        vec![false],
        "a page holds only what the body drew, so a body under a path clip's coverage mask is          never retained"
    );

    // Exactly a rounded clip's terms: the two clips are the same lowering.
    let under_rounded = compile_retained(&scene_of(|builder| {
        builder.push_clip_rounded(Rect::new(0.0, 0.0, 64.0, 48.0), 6.0);
        record_retained_body(builder);
        builder.pop_clip();
    }));
    assert_eq!(retainable(&under_path), retainable(&under_rounded));
}

#[test]
fn a_path_clip_opened_inside_a_retained_body_is_judged_like_a_rounded_one() {
    let with_inner = |clip: &dyn Fn(&mut SceneBuilder<'_>)| {
        compile_retained(&scene_of(|builder| {
            builder.push_transform(Affine::translate((8.0, 8.0)));
            builder.push_retained(1, RETAINED_BODY, 1.0, 1.0);
            clip(builder);
            builder.fill_rect(RETAINED_BODY, red());
            builder.pop_clip();
            builder.pop_retained();
            builder.pop_transform();
        }))
    };
    let path = with_inner(&|builder| {
        let mut triangle = BezPath::new();
        triangle.move_to((0.0, 0.0));
        triangle.line_to((16.0, 0.0));
        triangle.line_to((8.0, 16.0));
        triangle.close_path();
        builder.push_clip_path(triangle);
    });
    let rounded = with_inner(&|builder| builder.push_clip_rounded(RETAINED_BODY, 4.0));

    assert_eq!(path.mask_clips, 1);
    assert_eq!(
        retainable(&path),
        retainable(&rounded),
        "a mask the body opens itself is part of what the page holds, on a rounded clip's terms"
    );
}

#[test]
fn a_path_clip_opens_no_layer_and_no_page() {
    let frame = compile(&scene_of(|builder| {
        builder.push_clip_path(wide_outline());
        builder.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), red());
        builder.pop_clip();
    }));

    assert!(
        !frame.recorder.has_layers(),
        "a path clip is a coverage mask, never a recorded layer"
    );
    assert!(
        page_opacities(&rounds(&frame)).is_empty(),
        "and so it costs the scheduler no page"
    );
}

// ---------------------------------------------------------------------
// Nested retained brackets: keys the frame supersedes
// ---------------------------------------------------------------------

#[test]
fn a_nested_bracket_lists_its_key_as_superseded_and_records_no_bracket() {
    let frame = compile_retained(&scene_of(|builder| {
        builder.push_transform(Affine::translate((8.0, 8.0)));
        builder.push_retained(1, RETAINED_BODY, HALF, 1.0);
        builder.push_retained(2, RETAINED_BODY, 1.0, 1.0);
        builder.push_retained(3, Rect::new(0.0, 0.0, 8.0, 8.0), 0.25, 2.0);
        builder.fill_rect(RETAINED_BODY, red());
        builder.pop_retained();
        builder.pop_retained();
        builder.pop_retained();
        builder.pop_transform();
        // A sibling at the root is outermost again: listed, not superseded.
        builder.push_retained(4, Rect::new(30.0, 8.0, 46.0, 24.0), 1.0, 1.0);
        builder.fill_rect(Rect::new(30.0, 8.0, 46.0, 24.0), red());
        builder.pop_retained();
    }));

    let listed: Vec<u64> = frame.retained.iter().map(|bracket| bracket.key).collect();
    assert_eq!(listed, vec![1, 4], "only outermost brackets are recorded");
    assert_eq!(
        frame.retained_superseded,
        vec![2, 3],
        "every nested key is reported, in recording order, so the store can evict \
         whatever page it held — the body painted into the outermost one instead"
    );
}

#[test]
fn a_bracket_off_the_grid_is_superseded_too() {
    let rect = Rect::new(0.0, 0.0, 16.0, 16.0);
    let frame = compile_retained(&scene_of(|builder| {
        builder.push_retained(1, rect, 1.0, 1e200);
        // Composed with the outer bracket's correction, this one's transform
        // leaves the device grid: it is entered neutrally and never listed.
        builder.push_transform(Affine::scale(1e200));
        builder.push_retained(2, rect, 1.0, 1.0);
        builder.pop_retained();
        builder.pop_transform();
        builder.pop_retained();
    }));
    let listed: Vec<u64> = frame.retained.iter().map(|bracket| bracket.key).collect();
    assert_eq!(listed, vec![1]);
    assert_eq!(frame.retained_superseded, vec![2]);
}

#[test]
fn without_retention_no_key_is_superseded() {
    let frame = compile(&scene_of(|builder| {
        builder.push_retained(1, RETAINED_BODY, HALF, 1.0);
        builder.push_retained(2, RETAINED_BODY, HALF, 1.0);
        builder.fill_rect(RETAINED_BODY, red());
        builder.pop_retained();
        builder.pop_retained();
    }));
    assert!(frame.retained.is_empty());
    assert!(
        frame.retained_superseded.is_empty(),
        "a compiler with no store behind it has nothing to evict"
    );

    let flat = compile_retained(&scene_of(record_retained_body));
    assert_eq!(flat.retained.len(), 1);
    assert!(
        flat.retained_superseded.is_empty(),
        "a lone outermost bracket supersedes nothing"
    );
}

// ---------------------------------------------------------------------
// The blend module
// ---------------------------------------------------------------------

/// The blend module as a device would compile it: the binding-free helper
/// prelude followed by its own source, the same assembly every other engine
/// module uses.
fn blend_source() -> String {
    format!(
        "{}{}",
        include_str!("../shaders/helpers.wgsl"),
        include_str!("../shaders/blend.wgsl")
    )
}

#[test]
fn the_blend_module_parses_and_validates() {
    let source = blend_source();
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|err| panic!("the blend module failed to parse: {err:?}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|err| panic!("the blend module failed validation: {err:?}"));
}

#[test]
fn the_blend_module_carries_the_destination_out_composite() {
    let blend = include_str!("../shaders/blend.wgsl");
    assert!(
        blend.contains("const COMPOSE_DEST_OUT: u32 = 8u;"),
        "the punch's composite mode is what this module is ported for"
    );
    assert!(
        !blend.contains("var<storage") && !blend.contains("@compute"),
        "the module stays inside the downlevel design rules"
    );
}

// ---------------------------------------------------------------------
// A backdrop blur: never retained, never inside a page
// ---------------------------------------------------------------------

/// The rectangle every backdrop case below blurs behind, in its own space.
const PANEL: Rect = Rect::new(4.0, 4.0, 12.0, 12.0);

/// The rounds `frame` schedules as with its backdrop blurs served — the
/// scheduler entry `EngineRenderer::encode` takes on a readable target.
fn backdrop_rounds(frame: &CompiledFrame) -> Vec<Round> {
    Schedule::build_frame(
        &frame.recorder,
        &TierCaps::fake(DownlevelProfile::Full),
        &PageConfig::default(),
        &RetainedLayers::new(),
        &frame.backdrops,
    )
    .expect("a frame carrying a backdrop schedules")
}

/// Every draw index a round of `target`'s kind issues.
fn drawn_in(rounds: &[Round], pages: bool) -> Vec<u32> {
    rounds
        .iter()
        .filter(|round| !round.is_root() == pages)
        .flat_map(|round| round.ops.iter())
        .filter_map(|op| match op {
            RoundOp::Draws(range) => Some(range.clone()),
            RoundOp::Composite(_) => None,
        })
        .flatten()
        .collect()
}

/// Where the backdrop's own draw — its rectangle's coverage — lands, if
/// anywhere: `(in a surface round, in a page round, copied)`.
fn backdrop_placement(frame: &CompiledFrame) -> (bool, bool, bool) {
    let rounds = backdrop_rounds(frame);
    let Some(backdrop) = frame.backdrops.first() else {
        return (false, false, false);
    };
    let draw = frame
        .draws()
        .iter()
        .position(|draw| draw.depth == backdrop.depth)
        .and_then(|index| u32::try_from(index).ok())
        .expect("a recorded backdrop has its draw");
    (
        drawn_in(&rounds, false).contains(&draw),
        drawn_in(&rounds, true).contains(&draw),
        rounds.iter().any(|round| round.region_copy().is_some()),
    )
}

#[test]
fn a_retained_body_carrying_a_backdrop_blur_is_marked_unretainable() {
    let plain = compile_retained(&scene_of(record_retained_body));
    assert_eq!(retainable(&plain), vec![true], "the control");

    let blurring = compile_retained(&scene_of(|builder| {
        builder.push_transform(Affine::translate((8.0, 8.0)));
        builder.push_retained(1, RETAINED_BODY, 1.0, 1.0);
        builder.fill_rect(RETAINED_BODY, red());
        builder.backdrop_blur(PANEL, 2.0);
        builder.pop_retained();
        builder.pop_transform();
    }));
    assert_eq!(
        blurring.backdrops.len(),
        1,
        "the backdrop is still recorded"
    );
    assert_eq!(
        retainable(&blurring),
        vec![false],
        "a page is rasterized offscreen, where there is no frame beneath the body to sample, \
         so a body blurring its backdrop is drawn inline every frame"
    );
}

#[test]
fn a_backdrop_at_the_root_is_copied_and_drawn_in_a_surface_round() {
    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), red());
        builder.backdrop_blur(PANEL, 2.0);
    }));
    assert_eq!(backdrop_placement(&frame), (true, false, true));
}

#[test]
fn a_backdrop_never_lands_in_a_page_round() {
    // Inside a translucent layer: an isolated page, with no frame beneath.
    let in_layer = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), red());
        builder.push_layer(Rect::new(0.0, 0.0, 32.0, 32.0), HALF);
        builder.fill_rect(Rect::new(2.0, 2.0, 20.0, 20.0), red());
        builder.backdrop_blur(PANEL, 2.0);
        builder.pop_layer();
    }));
    assert_eq!(in_layer.backdrops.len(), 1);
    assert_eq!(
        backdrop_placement(&in_layer),
        (false, false, false),
        "dropped: no copy, and its rectangle drawn in no round at all"
    );

    // Inside a translucent retained body the store did not admit: the
    // ordinary isolated layer the inline lowering describes.
    let in_body = compile_retained(&scene_of(|builder| {
        builder.push_retained(1, Rect::new(0.0, 0.0, 32.0, 32.0), HALF, 1.0);
        builder.fill_rect(Rect::new(2.0, 2.0, 20.0, 20.0), red());
        builder.backdrop_blur(PANEL, 2.0);
        builder.pop_retained();
    }));
    assert_eq!(retainable(&in_body), vec![false]);
    assert_eq!(backdrop_placement(&in_body), (false, false, false));

    // A full-opacity unretainable body inlines into the surface, so its
    // backdrop is the frame's own and is served there — still never in a
    // page.
    let inlined = compile_retained(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), red());
        builder.push_retained(1, Rect::new(0.0, 0.0, 32.0, 32.0), 1.0, 1.0);
        builder.backdrop_blur(PANEL, 2.0);
        builder.pop_retained();
    }));
    assert_eq!(backdrop_placement(&inlined), (true, false, true));
}
