//! Retained pages, host-side: what a compiler with retention on records for a
//! `PushRetained`/`DrawRetained`, what the retained-page store decides for it
//! (admit, evict, refuse, serve, list), and what the scheduler makes of the
//! store's answers.
//!
//! No GPU is involved. The store is generic over its texture and view types,
//! so it is driven here against a counting fake allocator — the same shape the
//! intermediate pool's own tests use — and a page is a decision plus a counted
//! allocation. The pixels a retained page composites are pinned on hardware by
//! `frust-render`'s `gpu_smoke` and `frust-testing`'s `retained_parity` golden.
//!
//! The claims under test are the store's whole contract: a body is admitted
//! only when a page can hold what it really drew, a `DrawRetained` is served
//! from a page with the same linear transform at a whole-pixel offset and at
//! any alpha, one that cannot be served refuses the frame with
//! [`EngineError::RetainedKeyMissing`] and empties the store, the budget is
//! kept by evicting least recently used pages and never one the frame uses, a
//! refused frame leaves the store as it was, and a retained page never counts
//! toward the scheduler's live-page bound.

use std::cell::Cell;

use frust_engine::cache::images::AtlasTier;
use frust_engine::cache::retained::{
    RetainedBudget, RetainedDecision, RetainedPages, RetainedPlan, RetainedRefusal,
};
use frust_engine::compile::CompiledFrame;
use frust_engine::schedule::{
    MAX_LIVE_PAGES, PageConfig, RetainedLayers, RetainedRole, Round, RoundOp, RoundTarget, Schedule,
};
use frust_engine::{EngineDraw, EngineError, RetainedKind, SceneCompiler};
use frust_gpu::{DownlevelProfile, TextureAllocator, TextureDesc, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::Brush;
use peniko::color::palette::css::{BLUE, RED};
use vello_common::color::palette::css::RED as RAW_RED;
use vello_common::geometry::RectU16;
use vello_common::paint::Paint;
use vello_common::record::{CommandRecorder, LayerProps};
use vello_common::strip::Strip;
use vello_common::tile::Tile;

/// Viewport every compiled case uses. Not square, so a swapped axis cannot
/// pass by symmetry.
const VIEWPORT: (u16, u16) = (128, 96);

/// A body's rectangle in its own space: 32x24, placed by the transform below.
const BODY: Rect = Rect::new(0.0, 0.0, 32.0, 24.0);

/// Bytes one page of [`BODY`] costs at 1x, recorded below full opacity —
/// where the body is clipped to its rectangle, so the page is exactly it.
/// (At full opacity the page takes the body's strip bounds, which run a tile
/// past a fill ending on a tile edge.)
const BODY_BYTES: u64 = 32 * 24 * 4;

/// An alpha below full opacity, so a recorded body's page is exactly its
/// rectangle.
const HALF: f32 = 0.5;

fn caps() -> TierCaps {
    TierCaps::fake(DownlevelProfile::Full)
}

/// A texture allocator that hands out numbered fakes and counts them.
#[derive(Default)]
struct FakeAllocator {
    allocated: Cell<u32>,
}

impl TextureAllocator for FakeAllocator {
    type Texture = (u32, u32);
    type View = u32;

    fn allocate_texture(&self, desc: &TextureDesc) -> ((u32, u32), u32) {
        let id = self.allocated.get() + 1;
        self.allocated.set(id);
        ((desc.width, desc.height), id)
    }
}

type FakePages = RetainedPages<(u32, u32), u32>;

fn store(budget: u64) -> FakePages {
    RetainedPages::new(RetainedBudget { bytes: budget }, 8192)
}

fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

fn retaining_compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_retained_layers(true);
    compiler
}

fn compile_retained(scene: &Scene) -> CompiledFrame {
    retaining_compiler()
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// A `PushRetained` of [`BODY`] under `at`, filled solid.
fn record_body(builder: &mut SceneBuilder<'_>, key: u64, at: Affine, alpha: f32) {
    builder.push_transform(at);
    builder.push_retained(key, BODY, alpha, 1.0);
    builder.fill_rect(BODY, Brush::Solid(RED));
    builder.pop_retained();
    builder.pop_transform();
}

/// A `DrawRetained` of [`BODY`] under `at`.
fn draw_body(builder: &mut SceneBuilder<'_>, key: u64, at: Affine, alpha: f32, scale: f64) {
    builder.push_transform(at);
    builder.draw_retained(key, BODY, alpha, scale);
    builder.pop_transform();
}

/// Plan `frame` against `pages` and commit it as if every admitted body
/// rendered.
fn run(pages: &mut FakePages, allocator: &FakeAllocator, mut frame: CompiledFrame) -> RetainedPlan {
    let plan = pages
        .plan(&frame.retained, &mut frame.recorder.layers)
        .expect("the frame's draws are all served");
    let rendered = vec![true; frame.retained.len()];
    pages.commit(allocator, &frame.retained, &plan, &rendered);
    plan
}

fn keys(pages: &FakePages) -> Vec<u64> {
    let mut keys: Vec<u64> = pages.entries().map(|entry| entry.key).collect();
    keys.sort_unstable();
    keys
}

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

// ---------------------------------------------------------------------
// What the compiler records
// ---------------------------------------------------------------------

#[test]
fn a_compiler_without_a_store_lowers_inline_and_refuses_a_draw_by_its_key() {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    assert!(!compiler.retained_layers_enabled());

    let frame = compiler
        .compile(
            &scene_of(|builder| record_body(builder, 3, Affine::translate((8.0, 8.0)), 1.0)),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("a bracket compiles");
    assert!(frame.retained.is_empty());
    assert!(
        frame.recorder.layers.is_empty(),
        "a full-opacity bracket lowers to nothing at all without a store"
    );

    let refused = compiler.compile(
        &scene_of(|builder| draw_body(builder, 9, Affine::IDENTITY, 1.0, 1.0)),
        Affine::IDENTITY,
        VIEWPORT,
    );
    assert!(
        matches!(refused, Err(EngineError::RetainedKeyMissing { key: 9 })),
        "a DrawRetained reaching a compiler that retains nothing is the dedicated error: {refused:?}"
    );
}

#[test]
fn with_retention_on_every_outermost_bracket_is_a_layer_listed_for_the_store() {
    let frame = compile_retained(&scene_of(|builder| {
        record_body(builder, 1, Affine::translate((8.0, 8.0)), 1.0);
        builder.push_transform(Affine::translate((48.0, 40.0)) * Affine::scale(2.0));
        builder.push_retained(2, Rect::new(0.0, 0.0, 16.0, 12.0), 0.5, 1.0);
        // Nested: only the outermost bracket is honoured.
        builder.push_retained(3, Rect::new(0.0, 0.0, 4.0, 4.0), 0.25, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, 16.0, 12.0), Brush::Solid(BLUE));
        builder.pop_retained();
        builder.pop_retained();
        builder.pop_transform();
    }));

    let recorded: Vec<(u64, f32, (u32, u32), f32)> = frame
        .retained
        .iter()
        .map(|bracket| {
            (
                bracket.key,
                bracket.opacity,
                bracket.extent_px,
                bracket.placement.scale(),
            )
        })
        .collect();
    assert_eq!(
        recorded,
        vec![(1, 1.0, (32, 24), 1.0), (2, 0.5, (32, 24), 2.0)],
        "one entry per outermost bracket, its extent and scale as rasterized"
    );
    assert_eq!(
        frame.recorder.layers.len(),
        2,
        "each body is a layer of its own"
    );
    assert_eq!(retainable(&frame), vec![true, true]);
    match frame.retained[0].kind {
        RetainedKind::Record { rect, .. } => {
            assert_eq!(
                rect,
                RectU16::new(8, 8, 40, 32),
                "the rectangle, rounded out to tiles"
            );
        }
        RetainedKind::Draw { .. } => panic!("a PushRetained is a record"),
    }
}

#[test]
fn a_full_opacity_retained_body_draws_exactly_the_inline_coverage() {
    let scene = scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 128.0, 96.0), Brush::Solid(BLUE));
        builder.push_transform(Affine::translate((10.5, 7.25)));
        builder.push_retained(5, BODY, 1.0, 0.75);
        builder.fill_rect(Rect::new(2.0, 2.0, 30.0, 20.0), Brush::Solid(RED));
        builder.pop_retained();
        builder.pop_transform();
    });
    let inline = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let retained = compile_retained(&scene);

    assert_eq!(retained.strip_buf(), inline.strip_buf());
    assert_eq!(retained.alphas(), inline.alphas());
    let depths = |frame: &CompiledFrame| -> Vec<u32> {
        frame
            .draws()
            .iter()
            .map(|draw: &EngineDraw| draw.depth)
            .collect()
    };
    assert_eq!(depths(&retained), depths(&inline));
}

#[test]
fn a_draw_retained_is_an_empty_layer_at_its_painter_order_slot() {
    let frame = compile_retained(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Brush::Solid(RED));
        draw_body(builder, 4, Affine::translate((16.0, 16.0)), 0.5, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Brush::Solid(BLUE));
    }));

    assert_eq!(frame.recorder.layers.len(), 1);
    assert!(
        frame.recorder.layers[0].nodes.is_empty(),
        "the layer draws nothing itself"
    );
    let draw_depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    assert_eq!(draw_depths, vec![0, 2]);
    match frame.retained[0].kind {
        RetainedKind::Draw {
            depth,
            window,
            masked,
        } => {
            assert_eq!(
                depth, 1,
                "the composite takes the slot between the two fills"
            );
            assert_eq!(window, RectU16::new(0, 0, VIEWPORT.0, VIEWPORT.1));
            assert!(!masked);
        }
        RetainedKind::Record { .. } => panic!("a DrawRetained is a draw"),
    }
}

#[test]
fn bodies_a_page_cannot_hold_are_marked_unretainable() {
    let at = Affine::translate((8.0, 8.0));
    let cases: Vec<(&str, Scene, bool)> = vec![
        (
            "a plain body",
            scene_of(|builder| record_body(builder, 1, at, 1.0)),
            true,
        ),
        (
            "a body inside a scissor that holds it",
            scene_of(|builder| {
                builder.push_clip(Rect::new(0.0, 0.0, 64.0, 64.0));
                record_body(builder, 1, at, 1.0);
                builder.pop_clip();
            }),
            true,
        ),
        (
            "a body reaching past the viewport",
            scene_of(|builder| record_body(builder, 1, Affine::translate((110.0, 8.0)), 1.0)),
            false,
        ),
        (
            "a body a scissor cuts",
            scene_of(|builder| {
                builder.push_clip(Rect::new(0.0, 0.0, 20.0, 20.0));
                record_body(builder, 1, at, 1.0);
                builder.pop_clip();
            }),
            false,
        ),
        (
            "a body under a coverage mask",
            scene_of(|builder| {
                builder.push_clip_rounded(Rect::new(0.0, 0.0, 64.0, 64.0), 6.0);
                record_body(builder, 1, at, 1.0);
                builder.pop_clip();
            }),
            false,
        ),
        (
            "a body that punches",
            scene_of(|builder| {
                builder.push_transform(at);
                builder.push_retained(1, BODY, 1.0, 1.0);
                builder.clear_rect(Rect::new(2.0, 2.0, 6.0, 6.0));
                builder.pop_retained();
                builder.pop_transform();
            }),
            false,
        ),
        (
            "a body drawing an external texture",
            scene_of(|builder| {
                builder.push_transform(at);
                builder.push_retained(1, BODY, 1.0, 1.0);
                builder.scene_texture(77, Rect::new(0.0, 0.0, 8.0, 8.0));
                builder.pop_retained();
                builder.pop_transform();
            }),
            false,
        ),
        (
            "a body a stray pop closed early",
            scene_of(|builder| {
                builder.push_transform(at);
                builder.push_retained(1, BODY, 0.5, 1.0);
                builder.pop_clip();
                builder.fill_rect(BODY, Brush::Solid(RED));
                builder.pop_retained();
                builder.pop_transform();
            }),
            false,
        ),
        (
            "a body left open at the end of the frame",
            scene_of(|builder| {
                builder.push_transform(at);
                builder.push_retained(1, BODY, 1.0, 1.0);
                builder.fill_rect(BODY, Brush::Solid(RED));
            }),
            false,
        ),
    ];

    for (what, scene, expected) in cases {
        let frame = compile_retained(&scene);
        assert_eq!(retainable(&frame), vec![expected], "{what}");
    }
}

#[test]
fn retention_keeps_the_parallel_walk_byte_identical() {
    let scene = scene_of(|builder| {
        for row in 0..6_u32 {
            let y = f64::from(row) * 14.0;
            builder.push_transform(Affine::translate((4.0, y + 1.5)));
            builder.push_retained(
                u64::from(row),
                Rect::new(0.0, 0.0, 120.0, 12.0),
                if row % 2 == 0 { 1.0 } else { 0.5 },
                1.0 + f64::from(row) * 0.01,
            );
            for column in 0..8_u32 {
                let x = f64::from(column) * 14.0 + 0.3;
                builder.fill_rounded_rect(
                    Rect::new(x, 0.5, x + 11.0, 11.0),
                    3.0,
                    Brush::Solid(RED),
                );
            }
            builder.pop_retained();
            builder.pop_transform();
        }
        draw_body(builder, 99, Affine::translate((3.0, 3.0)), 0.5, 1.0);
    });

    let mut sequential = retaining_compiler();
    sequential.set_parallel_workers(1);
    let mut parallel = retaining_compiler();
    parallel.set_parallel_workers(4);
    parallel.set_parallel_min_cost(0);

    let a = sequential
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = parallel
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    assert_eq!(a.strip_buf(), b.strip_buf());
    assert_eq!(a.alphas(), b.alphas());
    assert_eq!(a.retained, b.retained);
    assert_eq!(a.recorder.layers.len(), b.recorder.layers.len());
    assert!(
        b.parallel_draws > 0,
        "precondition: helpers really generated runs"
    );
}

// ---------------------------------------------------------------------
// The store: admit, serve, refuse, evict, list
// ---------------------------------------------------------------------

#[test]
fn a_recorded_body_becomes_resident_only_once_committed() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    let mut frame = compile_retained(&scene_of(|builder| {
        record_body(builder, 7, Affine::translate((8.0, 8.0)), 0.5);
    }));

    let plan = pages
        .plan(&frame.retained, &mut frame.recorder.layers)
        .expect("plans");
    assert_eq!(
        plan.decisions,
        vec![RetainedDecision::Record {
            bounds: RectU16::new(8, 8, 40, 32)
        }]
    );
    assert!(
        pages.is_empty(),
        "planning changes nothing — a frame refused after it must leave the store as it was"
    );
    let before = pages.generation();

    pages.commit(&allocator, &frame.retained, &plan, &[true]);
    let entries: Vec<_> = pages.entries().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, 7);
    assert_eq!(entries[0].extent_px, (32, 24));
    assert_eq!(entries[0].scale, 1.0);
    assert_eq!(allocator.allocated.get(), 1);
    assert_ne!(pages.generation(), before, "a new page is a ledger change");
    assert_eq!(pages.used_bytes(), BODY_BYTES);
    assert!(
        pages.page_for(0).is_some(),
        "the bracket is bound to its page"
    );
}

#[test]
fn only_a_body_the_schedule_rendered_is_committed() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    let mut frame = compile_retained(&scene_of(|builder| {
        record_body(builder, 7, Affine::translate((8.0, 8.0)), 1.0);
    }));
    let plan = pages
        .plan(&frame.retained, &mut frame.recorder.layers)
        .expect("plans");
    pages.commit(&allocator, &frame.retained, &plan, &[false]);
    assert!(pages.is_empty());
    assert_eq!(allocator.allocated.get(), 0);
}

#[test]
fn a_draw_is_served_at_any_alpha_and_any_whole_pixel_move() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| {
            record_body(builder, 1, Affine::translate((8.0, 8.0)), HALF);
        })),
    );

    let draw = |at: Affine, alpha: f32, pages: &mut FakePages| {
        let mut frame =
            compile_retained(&scene_of(|builder| draw_body(builder, 1, at, alpha, 1.0)));
        pages
            .plan(&frame.retained, &mut frame.recorder.layers)
            .expect("served")
            .decisions
    };

    assert_eq!(
        draw(Affine::translate((8.0, 8.0)), 0.25, &mut pages),
        vec![RetainedDecision::Draw {
            dest: RectU16::new(8, 8, 40, 32),
            origin: (0, 0),
        }],
        "where it was recorded, at another alpha"
    );
    assert_eq!(
        draw(Affine::translate((28.4, 9.0)), 1.0, &mut pages),
        vec![RetainedDecision::Draw {
            dest: RectU16::new(28, 9, 60, 33),
            origin: (0, 0),
        }],
        "moved by a translation rounded to whole pixels"
    );
    assert_eq!(
        draw(Affine::translate((-10.0, 8.0)), 1.0, &mut pages),
        vec![RetainedDecision::Draw {
            dest: RectU16::new(0, 8, 22, 32),
            origin: (10, 0),
        }],
        "trimmed to the viewport, sampling from the texel under the trimmed corner"
    );
    assert_eq!(
        draw(Affine::translate((500.0, 8.0)), 1.0, &mut pages),
        vec![RetainedDecision::Hidden],
        "moved wholly off screen: served, drawing nothing"
    );
}

#[test]
fn a_draw_the_store_cannot_serve_refuses_the_frame_and_clears_the_store() {
    let at = Affine::translate((8.0, 8.0));
    let cases: Vec<(&str, Scene, u64)> = vec![
        (
            "a key that was never recorded",
            scene_of(|builder| draw_body(builder, 2, at, 1.0, 1.0)),
            2,
        ),
        (
            "another presentation scale",
            scene_of(|builder| draw_body(builder, 1, at, 1.0, 0.9)),
            1,
        ),
        (
            "another device scale",
            scene_of(|builder| draw_body(builder, 1, at * Affine::scale(2.0), 1.0, 1.0)),
            1,
        ),
        (
            "a coverage mask around the draw",
            scene_of(|builder| {
                builder.push_clip_rounded(Rect::new(0.0, 0.0, 64.0, 64.0), 6.0);
                draw_body(builder, 1, at, 1.0, 1.0);
                builder.pop_clip();
            }),
            1,
        ),
    ];

    for (what, scene, key) in cases {
        let allocator = FakeAllocator::default();
        let mut pages = store(1 << 20);
        run(
            &mut pages,
            &allocator,
            compile_retained(&scene_of(|builder| record_body(builder, 1, at, 1.0))),
        );
        let generation = pages.generation();

        let mut frame = compile_retained(&scene);
        let refused = pages.plan(&frame.retained, &mut frame.recorder.layers);
        assert!(
            matches!(refused, Err(EngineError::RetainedKeyMissing { key: k }) if k == key),
            "{what}: {refused:?}"
        );
        assert!(pages.is_empty(), "{what}: the store is cleared");
        assert_ne!(pages.generation(), generation, "{what}: the ledger changed");
        assert!(pages.take_refusal(), "{what}: the refusal is reported once");
        assert!(!pages.take_refusal());
    }
}

#[test]
fn eviction_is_least_recently_used_and_spares_every_page_the_frame_uses() {
    let allocator = FakeAllocator::default();
    // Room for exactly two bodies.
    let mut pages = store(BODY_BYTES * 2);
    let at = |x: f64| Affine::translate((x, 8.0));

    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| record_body(builder, 1, at(0.0), HALF))),
    );
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| record_body(builder, 2, at(40.0), HALF))),
    );
    // Drawing 1 makes 2 the least recently used.
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| {
            draw_body(builder, 1, at(0.0), 1.0, 1.0)
        })),
    );
    let plan = run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| record_body(builder, 3, at(80.0), HALF))),
    );
    assert_eq!(plan.evictions, vec![2]);
    assert_eq!(keys(&pages), vec![1, 3]);
    assert!(pages.used_bytes() <= BODY_BYTES * 2);

    // A frame drawing both resident pages and recording a third cannot evict
    // either, so the third renders inline.
    let frame = compile_retained(&scene_of(|builder| {
        draw_body(builder, 1, at(0.0), 1.0, 1.0);
        draw_body(builder, 3, at(80.0), 1.0, 1.0);
        record_body(builder, 4, at(40.0), HALF);
    }));
    let plan = run(&mut pages, &allocator, frame);
    assert_eq!(
        plan.decisions[2],
        RetainedDecision::Inline(RetainedRefusal::OverBudget)
    );
    assert!(plan.evictions.is_empty());
    assert_eq!(keys(&pages), vec![1, 3]);
}

#[test]
fn refused_bodies_render_inline_and_stay_out_of_the_ledger() {
    let at = Affine::translate((8.0, 8.0));
    let cases: Vec<(&str, FakePages, Scene, RetainedRefusal)> = vec![
        (
            "a body larger than any page",
            RetainedPages::new(RetainedBudget { bytes: 1 << 20 }, 16),
            scene_of(|builder| record_body(builder, 1, at, 1.0)),
            RetainedRefusal::TooLarge,
        ),
        (
            "a body larger than the whole budget",
            store(BODY_BYTES - 1),
            scene_of(|builder| record_body(builder, 1, at, 1.0)),
            RetainedRefusal::OverBudget,
        ),
        (
            "a body drawing outside its rectangle",
            store(1 << 20),
            scene_of(|builder| {
                builder.push_transform(at);
                builder.push_retained(1, BODY, 1.0, 1.0);
                builder.fill_rect(Rect::new(0.0, 0.0, 60.0, 24.0), Brush::Solid(RED));
                builder.pop_retained();
                builder.pop_transform();
            }),
            RetainedRefusal::Overflow,
        ),
        (
            "a body drawing nothing",
            store(1 << 20),
            scene_of(|builder| {
                builder.push_transform(at);
                builder.push_retained(1, BODY, 1.0, 1.0);
                builder.pop_retained();
                builder.pop_transform();
            }),
            RetainedRefusal::Empty,
        ),
        (
            "a body clipped on the way in",
            store(1 << 20),
            scene_of(|builder| record_body(builder, 1, Affine::translate((110.0, 8.0)), 1.0)),
            RetainedRefusal::NotRetainable,
        ),
    ];

    for (what, mut pages, scene, reason) in cases {
        let allocator = FakeAllocator::default();
        let frame = compile_retained(&scene);
        let plan = run(&mut pages, &allocator, frame);
        assert_eq!(
            plan.decisions,
            vec![RetainedDecision::Inline(reason)],
            "{what}"
        );
        assert!(pages.is_empty(), "{what}: nothing resident");
        assert_eq!(allocator.allocated.get(), 0, "{what}: nothing allocated");
    }

    // The same key recorded twice in one frame: the second renders inline.
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    let frame = compile_retained(&scene_of(|builder| {
        record_body(builder, 1, Affine::translate((0.0, 0.0)), 1.0);
        record_body(builder, 1, Affine::translate((40.0, 40.0)), 1.0);
    }));
    let plan = run(&mut pages, &allocator, frame);
    assert!(matches!(plan.decisions[0], RetainedDecision::Record { .. }));
    assert_eq!(
        plan.decisions[1],
        RetainedDecision::Inline(RetainedRefusal::Duplicate)
    );
}

#[test]
fn a_re_recorded_body_reuses_its_page_while_it_fits() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    let at = Affine::translate((8.0, 8.0));
    let record = |builder: &mut SceneBuilder<'_>, width: f64| {
        builder.push_transform(at);
        builder.push_retained(1, BODY, HALF, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, width, 24.0), Brush::Solid(RED));
        builder.pop_retained();
        builder.pop_transform();
    };

    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| record(builder, 32.0))),
    );
    let generation = pages.generation();
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| record(builder, 16.0))),
    );
    assert_eq!(
        allocator.allocated.get(),
        1,
        "a smaller body reuses the page"
    );
    assert_eq!(
        pages.generation(),
        generation,
        "the same key at the same extent and scale changes nothing the ledger says"
    );

    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| {
            builder.push_transform(at);
            builder.push_retained(1, Rect::new(0.0, 0.0, 64.0, 24.0), HALF, 1.0);
            builder.fill_rect(Rect::new(0.0, 0.0, 64.0, 24.0), Brush::Solid(RED));
            builder.pop_retained();
            builder.pop_transform();
        })),
    );
    assert_eq!(
        allocator.allocated.get(),
        2,
        "a larger body takes a new page"
    );
    assert_eq!(pages.len(), 1, "and the old one goes");
    assert_eq!(pages.used_bytes(), 64 * 24 * 4);
}

#[test]
fn clearing_and_rebudgeting_change_what_the_ledger_lists() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| {
            record_body(builder, 1, Affine::translate((0.0, 0.0)), HALF);
            record_body(builder, 2, Affine::translate((40.0, 40.0)), HALF);
        })),
    );
    assert_eq!(keys(&pages), vec![1, 2]);

    pages.set_budget(RetainedBudget { bytes: BODY_BYTES });
    assert_eq!(pages.len(), 1, "a smaller budget evicts down to fit");

    let generation = pages.generation();
    pages.clear();
    assert!(pages.is_empty());
    assert_ne!(pages.generation(), generation);
    assert!(
        !pages.take_refusal(),
        "a clear the host asked for is not a refusal"
    );
}

#[test]
fn the_budget_follows_the_atlas_tier() {
    assert_eq!(
        RetainedBudget::for_tier(AtlasTier::Mobile),
        RetainedBudget::MOBILE
    );
    assert_eq!(
        RetainedBudget::for_tier(AtlasTier::MobileLarge),
        RetainedBudget::MOBILE_LARGE
    );
    assert_eq!(
        RetainedBudget::for_tier(AtlasTier::Desktop),
        RetainedBudget::DESKTOP
    );
    const { assert!(RetainedBudget::MOBILE.bytes < RetainedBudget::MOBILE_LARGE.bytes) };
    const { assert!(RetainedBudget::MOBILE_LARGE.bytes < RetainedBudget::DESKTOP.bytes) };
    // A full-screen 1080x2400 page, three times over, fits a phone.
    const { assert!(RetainedBudget::MOBILE.bytes >= 3 * 1080 * 2400 * 4) };
}

// ---------------------------------------------------------------------
// What the scheduler makes of the store's answers
// ---------------------------------------------------------------------

fn layer(opacity: f32) -> LayerProps {
    LayerProps {
        blend_mode: vello_common::peniko::BlendMode::default(),
        opacity,
        mask: None,
        clip_path: None,
    }
}

fn draw(recorder: &mut CommandRecorder<EngineDraw>, x: u16, y: u16, width: u16) {
    let strips = vec![
        Strip::new(x, y, 0, false),
        Strip::sentinel(y, u32::from(width) * u32::from(Tile::HEIGHT)),
    ];
    let depth = u32::try_from(recorder.draws.len()).expect("a handful of draws");
    recorder.push_draw(
        EngineDraw::new(Paint::from(RAW_RED), depth, 0..strips.len()),
        &strips,
    );
}

/// The fan-inside-a-fan `tests/schedule.rs` shows needing a fourth live page
/// (`a_sibling_pair_nested_inside_a_sibling_pair_escalates`): an outer layer
/// holding a flat child and a child carrying two children of its own.
fn nested_fan_inside_a_fan() -> CommandRecorder<EngineDraw> {
    let mut recorder = CommandRecorder::new(256, 192);
    recorder.push_layer(layer(0.5), None);
    recorder.push_layer(layer(0.5), None);
    draw(&mut recorder, 0, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(0.5), None);
    recorder.push_layer(layer(0.5), None);
    draw(&mut recorder, 48, 16, 32);
    recorder.pop_layer();
    recorder.push_layer(layer(0.5), None);
    draw(&mut recorder, 96, 16, 32);
    recorder.pop_layer();
    recorder.pop_layer();
    recorder.pop_layer();
    recorder
}

/// The pooled pages each round has live — its own and every one it samples —
/// which is what [`MAX_LIVE_PAGES`] bounds; the `tests/schedule.rs` live-page
/// assertion, unchanged. A retained round has no pooled page of its own, and a
/// retained composite releases none.
fn assert_pooled_pages_within_the_bound(rounds: &[Round]) {
    for round in rounds {
        let live = usize::from(round.page().is_some()) + round.released.len();
        assert!(
            live <= MAX_LIVE_PAGES,
            "no round holds more than the bound live: {round:?}"
        );
    }
}

#[test]
fn a_retained_page_does_not_count_toward_the_live_page_bound() {
    let recorder = nested_fan_inside_a_fan();
    let config = PageConfig::default();
    assert!(
        matches!(
            Schedule::build(&recorder, &caps(), &config),
            Err(EngineError::SchedulerEscalation { .. })
        ),
        "precondition: pooled throughout, the shape needs a fourth live page"
    );

    // The outer layer retained: it holds no page group, so the shape fits.
    let mut retained = RetainedLayers::new();
    retained.set(0, RetainedRole::Record { bracket: 0 });
    let rounds = Schedule::build_retained(&recorder, &caps(), &config, &retained)
        .expect("a retained outer layer takes no page group");
    assert_pooled_pages_within_the_bound(&rounds);

    let retained_rounds: Vec<_> = rounds.iter().filter_map(Round::retained).collect();
    assert!(!retained_rounds.is_empty());
    assert!(
        retained_rounds
            .iter()
            .all(|target| target.layer == 0 && target.bracket == 0)
    );
    assert!(
        rounds
            .iter()
            .all(|round| round.page().is_none_or(|page| page.layer != 0)),
        "the retained layer never takes a pooled page"
    );

    // Its composite samples the retained page and hands no group back.
    let root = rounds.last().expect("a root round");
    assert!(root.is_root());
    let composite = root
        .composites()
        .find(|composite| composite.layer == 0)
        .expect("the retained layer composites into the surface");
    assert_eq!(composite.retained.map(|source| source.bracket), Some(0));
    assert!(root.released.is_empty());

    // Every pooled layer is still composited exactly once.
    let mut composited: Vec<u32> = rounds
        .iter()
        .flat_map(Round::composites)
        .map(|c| c.layer)
        .collect();
    composited.sort_unstable();
    assert_eq!(composited, vec![0, 1, 2, 3, 4]);
}

#[test]
fn a_retained_body_is_isolated_at_full_opacity_and_rendered_at_zero() {
    let config = PageConfig::default();
    for (opacity, composites) in [(1.0, 1), (0.0, 0)] {
        let mut recorder = CommandRecorder::new(256, 192);
        recorder.push_layer(layer(opacity), None);
        draw(&mut recorder, 16, 16, 32);
        recorder.pop_layer();

        let mut retained = RetainedLayers::new();
        retained.set(0, RetainedRole::Record { bracket: 0 });
        let rounds =
            Schedule::build_retained(&recorder, &caps(), &config, &retained).expect("schedules");
        assert!(
            matches!(rounds[0].target, RoundTarget::Retained(_)),
            "opacity {opacity}: the body renders into its page"
        );
        assert_eq!(
            rounds.iter().flat_map(Round::composites).count(),
            composites,
            "opacity {opacity}"
        );

        // Without a role the same layer is the ordinary one: inlined at full
        // opacity, dropped at zero.
        let plain = Schedule::build(&recorder, &caps(), &config).expect("schedules");
        assert_eq!(plain.len(), 1);
        assert!(plain[0].is_root());
    }
}

#[test]
fn a_draw_role_is_a_composite_alone() {
    let mut recorder = CommandRecorder::new(256, 192);
    draw(&mut recorder, 0, 0, 16);
    recorder.push_layer(layer(0.5), None);
    recorder.pop_layer();
    draw(&mut recorder, 0, 8, 16);

    let dest = RectU16::new(40, 40, 72, 64);
    let mut retained = RetainedLayers::new();
    retained.set(
        0,
        RetainedRole::Draw {
            bracket: 3,
            dest,
            origin: (2, 0),
        },
    );
    let rounds = Schedule::build_retained(&recorder, &caps(), &PageConfig::default(), &retained)
        .expect("schedules");
    assert_eq!(rounds.len(), 1, "a draw renders nothing of its own");
    let ops: Vec<&RoundOp> = rounds[0].ops.iter().collect();
    assert_eq!(ops.len(), 3, "draws, the composite, draws: {ops:?}");
    let RoundOp::Composite(composite) = ops[1] else {
        panic!("the composite sits between the two draws: {ops:?}");
    };
    assert_eq!(composite.bounds, dest);
    assert_eq!(composite.source(), RectU16::new(2, 0, 34, 24));
    assert_eq!(composite.opacity, 0.5);
    assert!(rounds[0].released.is_empty());

    // Hidden: nothing at all.
    let mut hidden = RetainedLayers::new();
    hidden.set(0, RetainedRole::Hidden);
    let rounds = Schedule::build_retained(&recorder, &caps(), &PageConfig::default(), &hidden)
        .expect("schedules");
    assert_eq!(rounds[0].composites().count(), 0);
}

#[test]
fn a_key_the_frame_draws_is_not_re_recorded_in_the_same_frame() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    let at = Affine::translate((8.0, 8.0));
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| record_body(builder, 1, at, HALF))),
    );
    let plan = run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| {
            draw_body(builder, 1, at, HALF, 1.0);
            record_body(builder, 1, at, HALF);
        })),
    );
    assert!(matches!(plan.decisions[0], RetainedDecision::Draw { .. }));
    assert_eq!(
        plan.decisions[1],
        RetainedDecision::Inline(RetainedRefusal::Duplicate)
    );
    assert_eq!(allocator.allocated.get(), 1);
}

#[test]
fn a_served_draw_grows_every_layer_around_it_to_hold_its_composite() {
    let allocator = FakeAllocator::default();
    let mut pages = store(1 << 20);
    run(
        &mut pages,
        &allocator,
        compile_retained(&scene_of(|builder| {
            record_body(builder, 1, Affine::translate((8.0, 8.0)), HALF);
        })),
    );

    // A translucent layer holding nothing but the drawn page: its own draws
    // cover no pixel, so without the composite folded in it would be dropped.
    let mut frame = compile_retained(&scene_of(|builder| {
        builder.push_layer(Rect::new(0.0, 0.0, 128.0, 96.0), 0.5);
        draw_body(builder, 1, Affine::translate((50.0, 30.0)), 1.0, 1.0);
        builder.pop_layer();
    }));
    assert!(
        frame.recorder.layers[0].bbox.is_empty(),
        "precondition: the enclosing layer recorded no draw"
    );
    let plan = pages
        .plan(&frame.retained, &mut frame.recorder.layers)
        .expect("served");
    let RetainedDecision::Draw { dest, .. } = plan.decisions[0] else {
        panic!("the draw is served: {plan:?}");
    };
    assert_eq!(dest, RectU16::new(50, 30, 82, 54));
    assert_eq!(
        frame.recorder.layers[0].bbox,
        RectU16::new(48, 28, 84, 56),
        "the enclosing layer now covers the composite, rounded out to tiles"
    );

    let mut roles = RetainedLayers::new();
    roles.set(
        frame.retained[0].layer,
        RetainedRole::Draw {
            bracket: 0,
            dest,
            origin: (0, 0),
        },
    );
    let rounds = Schedule::build_retained(&frame.recorder, &caps(), &PageConfig::default(), &roles)
        .expect("schedules");
    let page = rounds
        .iter()
        .find_map(Round::page)
        .expect("the enclosing layer renders a page");
    assert_eq!(page.layer, 0);
    let composite = rounds[0]
        .composites()
        .next()
        .expect("the drawn page composites into the enclosing layer's page");
    assert_eq!(composite.bounds, dest);
}
