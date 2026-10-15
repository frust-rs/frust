//! Command-range projection over several ranges at once
//! (`frust_engine::renderer::project_command_ranges`) — the main surface's
//! segment of a segmented frame (render-model Phase 4, decision D10): every
//! command outside the overlays' cuts, each gap between two cuts projected as
//! its own self-contained display list.
//!
//! Device-free: the projection is a scene-to-scene rewrite, so every case
//! reads the projected scene's commands back.

use frust_engine::renderer::{
    RangeCut, RangeOptions, project_command_range, project_command_ranges,
};
use frust_scene::{Command, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::Brush;
use peniko::color::palette::css::{BLUE, GREEN, RED};

fn keep() -> RangeOptions {
    RangeOptions {
        keep_draw_retained: true,
    }
}

fn listing(scene: &Scene) -> Vec<String> {
    scene.commands().iter().map(|c| format!("{c:?}")).collect()
}

fn kinds(scene: &Scene) -> Vec<&'static str> {
    scene
        .commands()
        .iter()
        .map(|command| match command {
            Command::FillRect { .. } => "fill",
            Command::PushClip { .. } => "push_clip",
            Command::PopClip => "pop_clip",
            Command::PushLayer { .. } => "push_layer",
            Command::PopLayer => "pop_layer",
            Command::PushRetained { .. } => "push_retained",
            Command::PopRetained => "pop_retained",
            Command::DrawRetained { .. } => "draw_retained",
            Command::ClearRect { .. } => "clear",
            _ => "other",
        })
        .collect()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::new(x0, y0, x1, y1)
}

/// `[fill red, push_clip, fill green, clear, fill blue, clear, fill red,
/// pop_clip, fill blue]` — one clip open across two slot markers (indices 3
/// and 5), drawing between and around them.
fn two_slot_scene() -> Scene {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 64.0), Brush::Solid(RED));
    b.push_clip(rect(0.0, 0.0, 48.0, 64.0));
    b.fill_rect(rect(0.0, 0.0, 64.0, 8.0), Brush::Solid(GREEN));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(0.0, 16.0, 64.0, 24.0), Brush::Solid(BLUE));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(0.0, 32.0, 64.0, 40.0), Brush::Solid(RED));
    b.pop_clip();
    b.push_transform(Affine::translate((50.0, 50.0)));
    b.fill_rect(rect(0.0, 0.0, 8.0, 8.0), Brush::Solid(BLUE));
    b.pop_transform();
    scene
}

fn project(scene: &Scene, ranges: &[std::ops::Range<usize>]) -> (Scene, RangeCut) {
    let mut out = Scene::new();
    let cut = project_command_ranges(scene, ranges, keep(), &mut out);
    (out, cut)
}

#[test]
fn one_range_projects_exactly_what_the_single_range_projection_does() {
    let scene = two_slot_scene();
    for range in [0..9, 0..4, 3..6, 4..9, 9..9, 2..99] {
        let (many, many_cut) = project(&scene, std::slice::from_ref(&range));
        let mut one = Scene::new();
        let one_cut = project_command_range(&scene, range.clone(), keep(), &mut one);
        assert_eq!(listing(&many), listing(&one), "range {range:?}");
        assert_eq!(many_cut, one_cut, "range {range:?}");
    }
}

#[test]
fn several_ranges_concatenate_their_own_projections_in_order() {
    let scene = two_slot_scene();
    let ranges = [0..3, 5..6, 7..9];
    let (many, cut) = project(&scene, &ranges);
    let mut expected = Vec::new();
    let mut summed = RangeCut::default();
    for range in &ranges {
        let mut one = Scene::new();
        let part = project_command_range(&scene, range.clone(), keep(), &mut one);
        expected.extend(listing(&one));
        summed.reopened += part.reopened;
        summed.closed += part.closed;
        summed.dropped_pops += part.dropped_pops;
    }
    assert_eq!(listing(&many), expected);
    assert_eq!((cut.start, cut.end), (0, 9));
    assert_eq!(
        (cut.reopened, cut.closed, cut.dropped_pops),
        (summed.reopened, summed.closed, summed.dropped_pops)
    );
}

#[test]
fn a_clip_open_across_a_gap_closes_before_it_and_reopens_after_it() {
    let scene = two_slot_scene();
    // The main segment of a frame cut at both markers to the clip's end: the
    // head, then the tail.
    let (main, cut) = project(&scene, &[0..3, 7..9]);
    assert_eq!(
        kinds(&main),
        [
            "fill",
            "push_clip",
            "fill",
            "pop_clip",
            // The tail's own pop of the clip matches the reopened one.
            "push_clip",
            "pop_clip",
            "fill",
        ]
    );
    assert_eq!(cut.reopened, 1);
    assert_eq!(cut.closed, 1);
    assert_eq!(cut.dropped_pops, 0);
}

#[test]
fn no_range_projects_nothing() {
    let scene = two_slot_scene();
    let mut out = two_slot_scene();
    let cut = project_command_ranges(&scene, &[], keep(), &mut out);
    assert!(out.commands().is_empty(), "the output is reset first");
    assert_eq!(cut, RangeCut::default());
}

#[test]
fn a_retained_bracket_across_a_gap_is_painted_inline_on_both_sides() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.push_retained(7, rect(0.0, 0.0, 32.0, 32.0), 0.5, 1.0);
    b.fill_rect(rect(0.0, 0.0, 8.0, 8.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(8.0, 8.0, 16.0, 16.0), Brush::Solid(BLUE));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(16.0, 16.0, 24.0, 24.0), Brush::Solid(GREEN));
    b.pop_retained();

    let (main, cut) = project(&scene, &[0..2, 5..7]);
    // Each side opens the bracket's alpha as a plain layer, never a page.
    assert_eq!(
        kinds(&main),
        [
            "push_layer",
            "fill",
            "pop_layer",
            "push_layer",
            "fill",
            "pop_layer"
        ]
    );
    assert_eq!(cut.demoted_retained, 2);
    assert!(
        main.commands()
            .iter()
            .all(|c| !matches!(c, Command::PushRetained { .. })),
        "no half-page is recorded"
    );
}

#[test]
fn draw_retained_is_dropped_per_range_without_a_store() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.draw_retained(1, rect(0.0, 0.0, 4.0, 4.0), 1.0, 1.0);
    b.clear_rect(Rect::ZERO);
    b.draw_retained(2, rect(0.0, 0.0, 4.0, 4.0), 1.0, 1.0);

    let mut out = Scene::new();
    let cut = project_command_ranges(
        &scene,
        &[0..1, 2..3],
        RangeOptions {
            keep_draw_retained: false,
        },
        &mut out,
    );
    assert!(out.commands().is_empty());
    assert_eq!(cut.dropped_draw_retained, 2);

    let (kept, _) = project(&scene, &[0..1, 2..3]);
    assert_eq!(kinds(&kept), ["draw_retained", "draw_retained"]);
}
