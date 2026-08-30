//! Group lowering: what an opacity layer, a snapshot bracket and a hole punch
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
//! a snapshot bracket paints its body exactly as the equivalent
//! transform-and-layer pair would, and a clear is hoisted out of every bracket
//! around it and lowered to coverage that is exact at a tile-unaligned edge.
//!
//! Pixel comparison against the CPU reference renderer is a separate, GPU-bound
//! concern and lives with the phase's golden corpus, not here.

use core::ops::Range;

use frust_engine::compile::CompiledFrame;
use frust_engine::schedule::{PageConfig, Round, RoundTarget, Schedule};
use frust_engine::{EngineError, SceneCompiler};
use frust_gpu::{DownlevelProfile, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
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
// Snapshot brackets
// ---------------------------------------------------------------------

#[test]
fn a_snapshot_body_lowers_like_the_equivalent_transform_and_layer_pair() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);
    let body = Rect::new(12.0, 12.0, 36.0, 36.0);

    let snapshot = compile(&scene_of(|builder| {
        builder.push_snapshot(7, rect, HALF, 0.5);
        builder.fill_rect(body, red());
        builder.pop_snapshot();
    }));
    let equivalent = compile(&scene_of(|builder| {
        builder.push_transform(Affine::scale_about(0.5, rect.center()));
        builder.push_layer(rect, HALF);
        builder.fill_rect(body, red());
        builder.pop_layer();
        builder.pop_transform();
    }));

    assert_eq!(coverage(&snapshot), coverage(&equivalent));
    assert_eq!(
        page_opacities(&rounds(&snapshot)),
        page_opacities(&rounds(&equivalent))
    );
}

#[test]
fn a_snapshot_scales_its_body_about_the_rectangle_centre() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_snapshot(1, rect, 1.0, 0.5);
        builder.fill_rect(rect, red());
        builder.pop_snapshot();
    }));

    // Halved about (24, 24): every edge moves halfway to the centre.
    assert_eq!(covered_bbox(&coverage(&frame)), Some((16, 16, 32, 32)));
    assert!(
        frame.recorder.layers.is_empty(),
        "a bracket at full opacity needs no page"
    );
}

#[test]
fn a_snapshot_scales_and_fades_together() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.push_snapshot(2, rect, 0.25, 0.5);
        builder.fill_rect(rect, red());
        builder.pop_snapshot();
    }));

    assert_eq!(covered_bbox(&coverage(&frame)), Some((16, 16, 32, 32)));
    assert_eq!(page_opacities(&rounds(&frame)), vec![0.25]);
}

#[test]
fn only_the_outermost_snapshot_bracket_is_honoured() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);
    let inner = Rect::new(12.0, 12.0, 36.0, 36.0);

    let nested = compile(&scene_of(|builder| {
        builder.push_snapshot(1, rect, HALF, 0.5);
        builder.push_snapshot(2, inner, 0.25, 4.0);
        builder.fill_rect(rect, red());
        builder.pop_snapshot();
        builder.pop_snapshot();
    }));
    let outer_only = compile(&scene_of(|builder| {
        builder.push_snapshot(1, rect, HALF, 0.5);
        builder.fill_rect(rect, red());
        builder.pop_snapshot();
    }));

    assert_eq!(coverage(&nested), coverage(&outer_only));
    assert_eq!(nested.recorder.layers.len(), 1, "one bracket, one layer");
    assert_eq!(
        page_opacities(&rounds(&nested)),
        page_opacities(&rounds(&outer_only))
    );
}

#[test]
fn an_unbalanced_pop_snapshot_is_ignored() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        builder.pop_snapshot();
        builder.push_snapshot(1, rect, 1.0, 0.5);
        builder.pop_snapshot();
        builder.pop_snapshot();
        // The stray pops must leave no correction behind, so this paints where
        // it was recorded rather than scaled.
        builder.fill_rect(rect, red());
    }));

    assert_eq!(covered_bbox(&coverage(&frame)), Some((8, 8, 40, 40)));
    assert!(frame.recorder.layers.is_empty());
}

#[test]
fn a_snapshot_left_open_stops_correcting_at_the_end_of_the_frame() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let first = compile(&scene_of(|builder| {
        builder.push_snapshot(1, rect, HALF, 0.5);
        builder.fill_rect(rect, red());
    }));
    // The compiler holds no per-frame state between calls, so a second frame
    // through the same compiler must not inherit the first's correction.
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    let open = scene_of(|builder| {
        builder.push_snapshot(1, rect, HALF, 0.5);
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
        "the punch erases what was painted beneath it and nothing painted over it"
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
            "a snapshot scale",
            scene_of(|builder| {
                builder.push_snapshot(1, Rect::new(0.0, 0.0, 16.0, 16.0), 1.0, f64::NAN);
                builder.pop_snapshot();
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
fn a_singular_snapshot_transform_drops_the_scale_rather_than_the_frame() {
    let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

    let frame = compile(&scene_of(|builder| {
        // A collapsed transform has no usable inverse, so the presentation
        // scale cannot be conjugated through it.
        builder.push_transform(Affine::scale(0.0));
        builder.push_snapshot(1, rect, 1.0, 0.5);
        builder.pop_snapshot();
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
        builder.push_snapshot(1, rect, 1.0, 1e200);
        // On its own this transform is finite; composed with the bracket's
        // correction it is not, and no `u16` pixel could hold what it maps to.
        builder.push_transform(Affine::scale(1e200));
        builder.fill_rect(rect, red());
        builder.pop_transform();
        builder.pop_snapshot();
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
        builder.push_snapshot(1, rect, 1.0, 1e200);
        builder.push_transform(Affine::scale(1e200));
        // Composed with the bracket's correction this clip lands nowhere a
        // `u16` pixel could hold, so it admits nothing at all.
        builder.push_clip(rect);
        builder.fill_rect(rect, red());
        builder.pop_clip();
        builder.pop_transform();
        builder.pop_snapshot();
        builder.fill_rect(outside, red());
    }));

    assert_eq!(
        covered_bbox(&coverage(&frame)),
        Some((20, 20, 36, 36)),
        "the stray clip's pop closed the bracket that clip opened, not the frame"
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
