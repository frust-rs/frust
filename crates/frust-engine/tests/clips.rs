//! Clip lowering: what a `PushClip`/`PushClipRounded`/`PopClip` bracket costs,
//! and what it actually leaves painted.
//!
//! Every case drives the public seam — a `frust_scene::Scene` recorded through
//! `SceneBuilder`, compiled by [`SceneCompiler`] — and reads the result back
//! through the same pairwise strip walk the renderer uses, so the assertions
//! pin the pixels a GPU pass would produce rather than the compiler's
//! internals. No GPU, device, or surface is involved.
//!
//! The two claims under test are the ones the lowering exists for: a
//! rectangular clip on whole pixels costs no rasterization and no intermediate
//! target at all, and every other clip costs one coverage mask and still no
//! intermediate target.

use frust_engine::compile::CompiledFrame;
use frust_engine::{EngineError, SceneCompiler};
use frust_scene::{CornerRadii, Scene, SceneBuilder};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLUE, RED};
use peniko::{Brush, Color};
use vello_common::tile::Tile;

/// Viewport every case compiles against. Deliberately not square, so an axis
/// swapped somewhere in the rewrite cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (64, 48);

/// A clip rectangle aligned to whole pixels but to no tile or strip row on any
/// edge — the case the rewrite has to express by masking coverage rather than
/// by moving a strip.
const RAGGED_CLIP: Rect = Rect::new(13.0, 6.0, 51.0, 39.0);

fn solid(color: Color) -> Brush {
    Brush::Solid(color)
}

fn compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

/// A scene built by `record`, ready to compile.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

fn compile(scene: &Scene) -> CompiledFrame {
    compiler()
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// A closed diamond with no axis-aligned edge, so every draw of it carries
/// partial coverage the clip has to preserve exactly where it does not cut.
fn diamond() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((32.0, 4.0));
    path.line_to((60.0, 24.0));
    path.line_to((32.0, 44.0));
    path.line_to((4.0, 24.0));
    path.close_path();
    path
}

/// The frame's draws rasterized back into a coverage grid, one byte per pixel.
///
/// Reads the strip run exactly as `EngineRenderer`'s instance builder does — a
/// strip's own alpha-sampled span, plus the solid span filling the gap to the
/// next strip when the winding between them says there is one — so a rewrite
/// that produced a run the renderer would read differently shows up here as
/// wrong pixels rather than passing on a structural technicality.
fn coverage(frame: &CompiledFrame) -> Vec<u8> {
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
    for draw in frame.draws() {
        let run = strips
            .get(draw.strip_range.clone())
            .expect("a draw's strip range is inside the frame's strip buffer");
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

/// Whether the pixel at `(x, y)` is inside `rect`.
fn inside(rect: Rect, x: usize, y: usize) -> bool {
    let (x, y) = (x as f64, y as f64);
    x >= rect.x0 && x < rect.x1 && y >= rect.y0 && y < rect.y1
}

/// Assert `clipped` is `unclipped` with everything outside `rect` erased, and
/// nothing else changed.
fn assert_masked_by(clipped: &[u8], unclipped: &[u8], rect: Rect) {
    let width = usize::from(VIEWPORT.0);
    let mut painted = 0_u32;
    for (index, (&got, &whole)) in clipped.iter().zip(unclipped).enumerate() {
        let (x, y) = (index % width, index / width);
        let want = if inside(rect, x, y) { whole } else { 0 };
        assert_eq!(
            got, want,
            "pixel ({x}, {y}) is {got}, expected {want} (unclipped coverage {whole})"
        );
        painted += u32::from(got > 0);
    }
    assert!(
        painted > 0,
        "the clipped frame painted nothing at all, so the comparison proved nothing"
    );
}

// ---------------------------------------------------------------------------
// A rectangular clip on whole pixels: a scissor, costing nothing
// ---------------------------------------------------------------------------

#[test]
fn a_pixel_aligned_rectangular_clip_allocates_no_mask_and_no_intermediate() {
    let scene = scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
    });
    let frame = compile(&scene);

    assert_eq!(frame.scissor_clips, 1, "the clip should have scissored");
    assert_eq!(frame.mask_clips, 0);
    assert_eq!(
        frame.clip_mask_strips, 0,
        "a scissored clip rasterizes no mask strips"
    );
    assert!(
        !frame.recorder.has_layers(),
        "a clip must never open an intermediate target"
    );
    assert_eq!(frame.draws().len(), 1);
}

#[test]
fn a_scissor_erases_exactly_the_coverage_outside_its_rectangle() {
    let fill = Rect::new(0.0, 0.0, 64.0, 48.0);
    let unclipped = coverage(&compile(&scene_of(|b| b.fill_rect(fill, solid(RED)))));
    let clipped = coverage(&compile(&scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(fill, solid(RED));
        b.pop_clip();
    })));

    assert_masked_by(&clipped, &unclipped, RAGGED_CLIP);
}

#[test]
fn a_scissor_preserves_the_partial_coverage_it_does_not_cut() {
    let unclipped = coverage(&compile(&scene_of(|b| {
        b.fill_path(diamond(), solid(BLUE));
    })));
    let clipped = coverage(&compile(&scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_path(diamond(), solid(BLUE));
        b.pop_clip();
    })));

    assert_masked_by(&clipped, &unclipped, RAGGED_CLIP);
    assert!(
        unclipped.iter().any(|&value| value > 0 && value < 255),
        "the diamond must carry partial coverage for this case to mean anything"
    );
}

#[test]
fn a_scissored_stroke_keeps_its_antialiased_edges() {
    let unclipped = coverage(&compile(&scene_of(|b| {
        b.stroke_line(
            Point::new(6.0, 5.0),
            Point::new(58.0, 43.0),
            3.0,
            solid(RED),
        );
    })));
    let clipped = coverage(&compile(&scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.stroke_line(
            Point::new(6.0, 5.0),
            Point::new(58.0, 43.0),
            3.0,
            solid(RED),
        );
        b.pop_clip();
    })));

    assert_masked_by(&clipped, &unclipped, RAGGED_CLIP);
}

#[test]
fn a_draw_wholly_inside_a_scissor_is_left_byte_for_byte_alone() {
    let inner = Rect::new(20.0, 12.0, 40.0, 32.0);
    let bare = compile(&scene_of(|b| b.fill_rect(inner, solid(RED))));
    let clipped = compile(&scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(inner, solid(RED));
        b.pop_clip();
    }));

    assert_eq!(bare.strip_buf(), clipped.strip_buf());
    assert_eq!(bare.alphas(), clipped.alphas());
    assert_eq!(bare.fast_rect_draws, clipped.fast_rect_draws);
}

#[test]
fn a_scissored_fill_keeps_a_solid_interior_rather_than_becoming_all_coverage() {
    let scene = scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
    });
    let frame = compile(&scene);

    // Only the tiles and strip rows the clip's edges fall inside have to carry
    // coverage; the interior stays a solid span with no coverage at all. The
    // bound is the area the clip admits, which is what a rewrite that turned
    // every kept pixel into a coverage byte would have to exceed.
    let admitted = (RAGGED_CLIP.width() * RAGGED_CLIP.height()) as usize;
    assert!(
        frame.alphas().len() < admitted,
        "a {}-pixel clipped fill allocated {} coverage bytes — the solid interior was not kept",
        admitted,
        frame.alphas().len()
    );
}

#[test]
fn a_scissor_rewrite_leaves_every_strip_tile_aligned_and_sentinel_terminated() {
    let scene = scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.fill_path(diamond(), solid(BLUE));
        b.pop_clip();
    });
    let frame = compile(&scene);
    let strips = frame.strip_buf();

    assert!(!frame.draws().is_empty());
    for draw in frame.draws() {
        let run = strips
            .get(draw.strip_range.clone())
            .expect("a draw's strip range is inside the frame's strip buffer");
        assert!(
            run.len() >= 2,
            "a run is at least one strip and its sentinel"
        );
        assert!(
            run[run.len() - 1].is_sentinel(),
            "the renderer reads a strip's extent off the one after it"
        );

        for (index, pair) in run.windows(2).enumerate() {
            let (strip, next) = (pair[0], pair[1]);
            assert!(!strip.is_sentinel(), "strip {index} is a sentinel mid-run");
            assert_eq!(strip.x % Tile::WIDTH, 0, "strip {index} is off its tile");
            assert_eq!(
                strip.width_to(&next) % Tile::WIDTH,
                0,
                "strip {index} is not a whole number of tiles wide"
            );
            assert_eq!(strip.y % Tile::HEIGHT, 0, "strip {index} is off its row");
            let coverage_end = strip.alpha_idx() as usize
                + usize::from(strip.width_to(&next)) * usize::from(Tile::HEIGHT);
            assert!(
                coverage_end <= frame.alphas().len(),
                "strip {index} reads past the frame's coverage buffer"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Everything else: one coverage mask, still no intermediate
// ---------------------------------------------------------------------------

#[test]
fn a_rounded_clip_rasterizes_a_coverage_mask_and_no_intermediate() {
    let scene = scene_of(|b| {
        b.push_clip_rounded(Rect::new(8.0, 8.0, 56.0, 40.0), 10.0);
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
    });
    let frame = compile(&scene);

    assert_eq!(frame.mask_clips, 1);
    assert_eq!(frame.scissor_clips, 0);
    assert!(
        frame.clip_mask_strips > 0,
        "a rounded clip has to rasterize its own coverage"
    );
    assert!(
        !frame.recorder.has_layers(),
        "a rounded clip must never open an intermediate target"
    );
}

#[test]
fn a_rounded_clip_rounds_the_corners_it_paints() {
    let clip = Rect::new(8.0, 8.0, 56.0, 40.0);
    let scene = scene_of(|b| {
        b.push_clip_rounded(clip, 10.0);
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
    });
    let painted = coverage(&compile(&scene));
    let width = usize::from(VIEWPORT.0);
    let at = |x: usize, y: usize| painted.get(y * width + x).copied().unwrap_or(0);

    assert_eq!(at(32, 24), 255, "the middle of the clip stays painted");
    assert_eq!(
        at(9, 9),
        0,
        "the corner the radius cuts away is not painted"
    );
    assert_eq!(at(54, 38), 0, "nor is the opposite corner");
    assert_eq!(at(2, 24), 0, "nor is anything outside the clip rectangle");
    assert_eq!(at(32, 9), 255, "the straight edge between them is painted");
}

#[test]
fn a_square_cornered_rounded_clip_still_takes_the_scissor_path() {
    let scene = scene_of(|b| {
        b.push_clip_rounded_radii(RAGGED_CLIP, CornerRadii::uniform(0.0));
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
    });
    let frame = compile(&scene);

    assert_eq!(frame.scissor_clips, 1);
    assert_eq!(frame.clip_mask_strips, 0);
}

#[test]
fn a_rectangular_clip_off_the_pixel_grid_falls_back_to_a_mask() {
    let scene = scene_of(|b| {
        b.push_clip(Rect::new(12.5, 6.0, 51.0, 39.0));
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
    });
    let frame = compile(&scene);

    assert_eq!(frame.scissor_clips, 0);
    assert_eq!(frame.mask_clips, 1);
    assert!(frame.clip_mask_strips > 0);
}

#[test]
fn a_rotated_rectangular_clip_falls_back_to_a_mask() {
    let scene = scene_of(|b| {
        b.push_transform(Affine::rotate_about(0.4, Point::new(32.0, 24.0)));
        b.push_clip(Rect::new(12.0, 6.0, 52.0, 40.0));
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
        b.pop_transform();
    });
    let frame = compile(&scene);

    assert_eq!(frame.scissor_clips, 0);
    assert_eq!(frame.mask_clips, 1);
    assert!(!frame.recorder.has_layers());
}

// ---------------------------------------------------------------------------
// Nesting
// ---------------------------------------------------------------------------

/// Eight rectangular clips, each one pixel tighter than the last.
fn nested_rects() -> Vec<Rect> {
    (0..8)
        .map(|step| {
            let step = f64::from(step);
            Rect::new(8.0 + step, 4.0 + step, 56.0 - step, 44.0 - step)
        })
        .collect()
}

#[test]
fn eight_nested_rectangular_clips_intersect_to_the_innermost() {
    let rects = nested_rects();
    let innermost = rects.last().copied().expect("eight rectangles");
    let fill = Rect::new(0.0, 0.0, 64.0, 48.0);

    let unclipped = coverage(&compile(&scene_of(|b| b.fill_rect(fill, solid(RED)))));
    let clipped = coverage(&compile(&scene_of(|b| {
        for rect in &rects {
            b.push_clip(*rect);
        }
        b.fill_rect(fill, solid(RED));
        for _ in &rects {
            b.pop_clip();
        }
    })));

    assert_masked_by(&clipped, &unclipped, innermost);
}

#[test]
fn eight_nested_clips_stay_free_of_masks_when_every_one_is_a_rectangle() {
    let rects = nested_rects();
    let frame = compile(&scene_of(|b| {
        for rect in &rects {
            b.push_clip(*rect);
        }
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        for _ in &rects {
            b.pop_clip();
        }
    }));

    assert_eq!(frame.scissor_clips, 8);
    assert_eq!(frame.mask_clips, 0);
    assert_eq!(frame.clip_mask_strips, 0);
    assert!(!frame.recorder.has_layers());
}

#[test]
fn eight_nested_clips_mixing_both_lowerings_paint_inside_all_of_them() {
    let rects = nested_rects();
    let outermost = rects.first().copied().expect("eight rectangles");
    let innermost = rects.last().copied().expect("eight rectangles");

    let frame = compile(&scene_of(|b| {
        for (step, rect) in rects.iter().enumerate() {
            if step % 2 == 0 {
                b.push_clip(*rect);
            } else {
                b.push_clip_rounded(*rect, 4.0);
            }
        }
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        for _ in &rects {
            b.pop_clip();
        }
    }));

    assert_eq!(frame.scissor_clips, 4);
    assert_eq!(frame.mask_clips, 4);
    assert!(!frame.recorder.has_layers(), "still no intermediate target");

    let painted = coverage(&frame);
    let width = usize::from(VIEWPORT.0);
    for (index, &value) in painted.iter().enumerate() {
        let (x, y) = (index % width, index / width);
        if value > 0 {
            assert!(
                inside(innermost, x, y),
                "pixel ({x}, {y}) painted outside the innermost clip {innermost:?}"
            );
        }
        if !inside(outermost, x, y) {
            assert_eq!(value, 0, "pixel ({x}, {y}) painted outside every clip");
        }
    }
    assert!(painted.iter().any(|&value| value > 0), "nothing painted");
}

#[test]
fn a_pop_restores_the_enclosing_clip_rather_than_lifting_every_one() {
    let outer = Rect::new(8.0, 4.0, 56.0, 44.0);
    let inner = Rect::new(20.0, 12.0, 30.0, 20.0);
    let fill = Rect::new(0.0, 0.0, 64.0, 48.0);

    let unclipped = coverage(&compile(&scene_of(|b| b.fill_rect(fill, solid(RED)))));
    let after_pop = coverage(&compile(&scene_of(|b| {
        b.push_clip(outer);
        b.push_clip(inner);
        b.pop_clip();
        b.fill_rect(fill, solid(RED));
        b.pop_clip();
    })));

    assert_masked_by(&after_pop, &unclipped, outer);
}

// ---------------------------------------------------------------------------
// Degenerate brackets
// ---------------------------------------------------------------------------

#[test]
fn an_unbalanced_pop_is_ignored() {
    let fill = Rect::new(8.0, 8.0, 40.0, 40.0);
    let unclipped = coverage(&compile(&scene_of(|b| b.fill_rect(fill, solid(RED)))));

    let leading_pop = coverage(&compile(&scene_of(|b| {
        b.pop_clip();
        b.pop_clip();
        b.fill_rect(fill, solid(RED));
    })));
    assert_eq!(
        leading_pop, unclipped,
        "a pop with nothing to pop draws nothing away"
    );

    let over_popped = coverage(&compile(&scene_of(|b| {
        b.push_clip(Rect::new(0.0, 0.0, 64.0, 48.0));
        b.pop_clip();
        b.pop_clip();
        b.pop_clip();
        b.fill_rect(fill, solid(RED));
    })));
    assert_eq!(over_popped, unclipped);
}

#[test]
fn an_unpopped_clip_still_ends_with_the_frame() {
    let scene = scene_of(|b| {
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
    });
    let frame = compile(&scene);

    assert_eq!(frame.scissor_clips, 1);
    assert_eq!(frame.draws().len(), 1);

    // The next frame compiled by the same compiler must not inherit it.
    let mut compiler = compiler();
    let _ = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let after = compiler
        .compile(
            &scene_of(|b| b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED))),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("compiles");
    assert_eq!(after.scissor_clips, 0);
    assert_eq!(
        coverage(&after),
        coverage(&compile(&scene_of(|b| {
            b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        })))
    );
}

#[test]
fn a_clip_that_admits_nothing_records_no_draw() {
    let scene = scene_of(|b| {
        b.push_clip(Rect::new(4.0, 4.0, 20.0, 20.0));
        b.push_clip(Rect::new(40.0, 4.0, 56.0, 20.0));
        b.fill_rect(Rect::new(0.0, 0.0, 64.0, 48.0), solid(RED));
        b.pop_clip();
        b.pop_clip();
        b.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), solid(BLUE));
    });
    let frame = compile(&scene);

    assert_eq!(
        frame.draws().len(),
        1,
        "only the draw outside the empty clip survives"
    );
    assert_eq!(
        frame.draws()[0].depth,
        0,
        "a clipped-away draw consumes no depth"
    );
}

#[test]
fn a_clip_does_not_disturb_the_depths_of_the_draws_it_encloses() {
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), solid(RED));
        b.push_clip(RAGGED_CLIP);
        b.fill_rect(Rect::new(16.0, 16.0, 32.0, 32.0), solid(BLUE));
        b.pop_clip();
        b.fill_rect(Rect::new(40.0, 40.0, 48.0, 46.0), solid(RED));
    });
    let frame = compile(&scene);

    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    assert_eq!(depths, vec![0, 1, 2]);
}

// ---------------------------------------------------------------------------
// The totality contract: a clip's own numbers are load-bearing now
// ---------------------------------------------------------------------------

#[test]
fn a_non_finite_clip_rectangle_refuses_the_frame() {
    for rect in [
        Rect::new(f64::NAN, 0.0, 16.0, 16.0),
        Rect::new(0.0, 0.0, f64::INFINITY, 16.0),
    ] {
        let scene = scene_of(|b| {
            b.push_clip(rect);
            b.fill_rect(Rect::new(0.0, 0.0, 16.0, 16.0), solid(RED));
            b.pop_clip();
        });
        assert!(
            matches!(
                compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
                Err(EngineError::InvalidGeometry)
            ),
            "a clip is lowered now, so {rect:?} is geometry the frame is refused for"
        );
    }
}

#[test]
fn a_non_finite_clip_radius_refuses_the_frame() {
    let scene = scene_of(|b| {
        b.push_clip_rounded(Rect::new(4.0, 4.0, 40.0, 40.0), f64::NAN);
        b.fill_rect(Rect::new(0.0, 0.0, 16.0, 16.0), solid(RED));
        b.pop_clip();
    });

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

#[test]
fn a_non_finite_clip_transform_still_refuses_the_frame() {
    let scene = scene_of(|b| {
        b.push_transform(Affine::scale(f64::INFINITY));
        b.push_clip(Rect::new(4.0, 4.0, 40.0, 40.0));
        b.pop_clip();
        b.pop_transform();
    });

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidTransform)
    ));
}

#[test]
fn an_enormous_clip_rectangle_compiles_and_clips_nothing() {
    let fill = Rect::new(8.0, 8.0, 40.0, 40.0);
    let unclipped = coverage(&compile(&scene_of(|b| b.fill_rect(fill, solid(RED)))));
    let enormous = coverage(&compile(&scene_of(|b| {
        b.push_clip(Rect::new(-1e30, -1e30, 1e30, 1e30));
        b.fill_rect(fill, solid(RED));
        b.pop_clip();
    })));

    assert_eq!(enormous, unclipped);
}
