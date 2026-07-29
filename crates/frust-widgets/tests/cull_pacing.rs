//! Integration test: proves the `TickClass` aggregation and the paint-time
//! visible-rect culling compose correctly through a *real* `Flex` — an
//! offscreen paced loop's `request_frame_class` never bubbles out of the
//! container's `PaintCtx` at all (not merely "gets paced"), and scrolling it
//! back into the viewport resumes the bubble on the very next paint.
//!
//! `crates/frust-shell-common/tests/pacing_integration.rs` is the companion
//! piece: it composes the resulting `PaintOutcome` aggregation
//! with `FrameGate` at the shell layer, treating "offscreen
//! (culled)" as the fact this file proves — no request bubbles at all.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, HeroFrames, LayoutCtx, PaintCtx, PaintScene, TickClass,
    View, Widget, any,
};
use frust_widgets::{Column, FlexView};
use kurbo::{Point, Rect, Size};

/// A no-op paint scene — this suite only cares about the frame-request
/// side effects of painting, never the emitted draw commands.
struct NullScene;

impl PaintScene for NullScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

/// A perpetual decorative-loop stand-in: on every paint it bumps a shared
/// counter (so a test can prove whether it was painted at all this frame —
/// i.e. whether `Flex`'s culling reached it) and requests a continuation
/// frame of `class` (`CosmeticLoop` for a pacable loop, `Transition` for a
/// concurrent page transition/fling).
struct LoopWidget {
    class: TickClass,
    size: Size,
    paint_count: Rc<Cell<u32>>,
}

impl Widget for LoopWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.paint_count.set(self.paint_count.get() + 1);
        ctx.request_frame_class(self.class);
    }
}

struct LoopView {
    class: TickClass,
    size: Size,
    paint_count: Rc<Cell<u32>>,
}

impl View<()> for LoopView {
    type Element = LoopWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LoopWidget {
        LoopWidget {
            class: self.class,
            size: self.size,
            paint_count: self.paint_count.clone(),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut LoopWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.class = self.class;
        ChangeFlags::NONE
    }
}

const ROW_W: f64 = 100.0;
const ROW_H: f64 = 100.0;
const ROWS: usize = 5;

/// Build+layout a `Column` of `ROWS` `CosmeticLoop` rows stacked vertically
/// (`ROW_H` each — the same geometry `frust-widgets/src/flex.rs`'s own
/// culling unit tests use), returning the laid-out widget plus one
/// paint-count `Rc<Cell<u32>>` per row (index-aligned) so a test can tell
/// exactly which rows a given paint pass actually reached.
fn build_column(classes: [TickClass; ROWS]) -> (FlexWidgetHandle, [Rc<Cell<u32>>; ROWS]) {
    let counts: [Rc<Cell<u32>>; ROWS] = std::array::from_fn(|_| Rc::new(Cell::new(0)));
    let view: FlexView<()> = Column(
        classes
            .iter()
            .zip(counts.iter())
            .map(|(class, count)| {
                any::<(), _>(LoopView {
                    class: *class,
                    size: Size::new(ROW_W, ROW_H),
                    paint_count: count.clone(),
                })
            })
            .collect(),
    );
    let mut counter = 0u64;
    let mut ctx = BuildCtx::new(&mut counter);
    let mut w = view.build(&mut ctx);
    let mut lctx = LayoutCtx::new();
    w.layout(
        &mut lctx,
        &BoxConstraints::loose(Size::new(ROW_W, ROW_H * ROWS as f64)),
    );
    (FlexWidgetHandle(w), counts)
}

/// Thin newtype so the helper above doesn't have to name `frust_widgets`'
/// crate-private `FlexWidget` field layout — only that it implements
/// `Widget`, which is all a paint pass needs.
struct FlexWidgetHandle(frust_widgets::FlexWidget);

impl FlexWidgetHandle {
    fn paint_at(&mut self, visible_rect: Option<Rect>) -> PaintClass {
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * ROWS as f64));
        if let Some(vr) = visible_rect {
            pctx.constrain_visible_rect(vr);
        }
        self.0.paint(&mut pctx, &mut scene);
        PaintClass {
            needs_frame: pctx.needs_frame(),
            needs_frame_paced_only: pctx.needs_frame_paced_only(),
            class: pctx.frame_class(),
        }
    }

    /// Like [`Self::paint_at`], but paints with a shared-element ("hero")
    /// reporter installed over the subtree — the `ctx.hero_active()` signal
    /// `Flex` reads to exempt every child from culling while a hero morph is in
    /// flight. Frame requests made inside bubble back out via
    /// `with_hero_registry`, so the returned [`PaintClass`] is read the same way.
    fn paint_at_with_hero(&mut self, visible_rect: Option<Rect>) -> PaintClass {
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * ROWS as f64));
        if let Some(vr) = visible_rect {
            pctx.constrain_visible_rect(vr);
        }
        let registry = RefCell::new(HeroFrames::new(Point::ZERO, HashMap::new()));
        pctx.with_hero_registry(&registry, |cctx| {
            self.0.paint(cctx, &mut scene);
        });
        PaintClass {
            needs_frame: pctx.needs_frame(),
            needs_frame_paced_only: pctx.needs_frame_paced_only(),
            class: pctx.frame_class(),
        }
    }
}

/// The aggregate `PaintCtx` frame-request state a real shell would latch
/// into next frame's `FrameInputs` (mirrors `frust_core::PaintOutcome`).
#[derive(Debug, Clone, Copy)]
struct PaintClass {
    needs_frame: bool,
    needs_frame_paced_only: bool,
    class: Option<TickClass>,
}

fn all_cosmetic() -> [TickClass; ROWS] {
    [TickClass::CosmeticLoop; ROWS]
}

/// Rows 0/1 fall inside the top-100px viewport (+1-viewport warm margin
/// reaches down to y=200, so row 2 stays warm too); rows 3/4 (y=300/400)
/// are fully outside and culled — the exact geometry
/// `flex.rs::culls_children_fully_outside_visible_rect_plus_margin` proves
/// at the widget layer; this file proves the same cull suppresses the
/// bubbled `TickClass` request.
fn top_viewport() -> Rect {
    Rect::from_origin_size(Point::ZERO, Size::new(ROW_W, ROW_H))
}

/// A viewport far below every row — every row is culled.
fn far_away_viewport() -> Rect {
    Rect::from_origin_size(Point::new(0.0, 10_000.0), Size::new(ROW_W, ROW_H))
}

#[test]
fn onscreen_paced_loops_bubble_a_cosmetic_loop_request() {
    let (mut w, counts) = build_column(all_cosmetic());
    let outcome = w.paint_at(Some(top_viewport()));

    // Rows 0/1/2 are within the warm band and must have painted (and thus
    // requested a continuation) exactly once; rows 3/4 must not have painted
    // at all — culling suppresses the paint call itself, not just the
    // resulting request.
    for (i, count) in counts.iter().enumerate() {
        let painted = count.get();
        if i <= 2 {
            assert_eq!(painted, 1, "row {i} is within the warm band and must paint");
        } else {
            assert_eq!(
                painted, 0,
                "row {i} is fully offscreen and must never paint"
            );
        }
    }

    assert!(
        outcome.needs_frame,
        "at least one onscreen loop requested a frame"
    );
    assert!(
        outcome.needs_frame_paced_only,
        "every request this paint was CosmeticLoop, so the aggregate must be paced-only"
    );
    assert_eq!(outcome.class, Some(TickClass::CosmeticLoop));
}

#[test]
fn fully_offscreen_column_bubbles_no_frame_request_at_all() {
    // The frame-suppression case this file exists for: a perpetual animator
    // entirely below the fold must leave the root `PaintOutcome` with no
    // frame request whatsoever — not "paced to zero", but genuinely none.
    let (mut w, counts) = build_column(all_cosmetic());
    let outcome = w.paint_at(Some(far_away_viewport()));

    for (i, count) in counts.iter().enumerate() {
        assert_eq!(count.get(), 0, "row {i} is offscreen and must never paint");
    }
    assert!(
        !outcome.needs_frame,
        "a fully-offscreen animator must bubble no frame request"
    );
    assert!(!outcome.needs_frame_paced_only);
    assert_eq!(outcome.class, None);
}

#[test]
fn scrolling_back_into_view_resumes_the_bubbled_request_on_the_very_next_paint() {
    let (mut w, counts) = build_column(all_cosmetic());

    // First paint: scrolled away — nothing bubbles, nothing paints.
    let away = w.paint_at(Some(far_away_viewport()));
    assert!(!away.needs_frame);
    assert!(counts.iter().all(|c| c.get() == 0));

    // A scroll event moves the viewport back to the top; the very next paint
    // (the shell's normal per-frame re-paint, forced to run by the scroll
    // event itself — never a culling concern) must resume both painting the
    // now-visible rows and bubbling their paced request.
    let back = w.paint_at(Some(top_viewport()));
    assert!(
        back.needs_frame,
        "frame requests resume once scrolled back into view"
    );
    assert!(back.needs_frame_paced_only);
    assert_eq!(counts[0].get(), 1, "row 0 painted on the resumed frame");
    assert_eq!(counts[1].get(), 1, "row 1 painted on the resumed frame");
}

#[test]
fn a_concurrent_onscreen_transition_dominates_an_onscreen_paced_loop() {
    // Row 0 is a CosmeticLoop, row 1 is a Transition, both within the warm
    // band — the aggregate must be Transition (the max-lattice rule), i.e.
    // never paced, exactly like a page transition playing over a shimmering
    // skeleton row.
    let mut classes = all_cosmetic();
    classes[1] = TickClass::Transition;
    let (mut w, counts) = build_column(classes);
    let outcome = w.paint_at(Some(top_viewport()));

    assert!(outcome.needs_frame);
    assert!(
        !outcome.needs_frame_paced_only,
        "any concurrent Transition request must un-pace the whole frame"
    );
    assert_eq!(outcome.class, Some(TickClass::Transition));
    assert_eq!(counts[0].get(), 1);
    assert_eq!(counts[1].get(), 1);
}

#[test]
fn a_hero_transition_in_flight_exempts_every_child_from_culling() {
    // A hero-tagged descendant scrolled past the warm band
    // stops reporting its morph bounds (`report_hero`) if culled. While a hero
    // transition is in flight (a reporter installed over the subtree), `Flex`
    // exempts EVERY child from culling — so the far-offscreen rows that
    // `fully_offscreen_column_bubbles_no_frame_request_at_all` proves are culled
    // *without* a hero all paint again here. This bypasses the
    // offscreen-animator suppression above BY DESIGN, and only for the brief transition.
    let (mut w, counts) = build_column(all_cosmetic());
    let outcome = w.paint_at_with_hero(Some(far_away_viewport()));

    for (i, count) in counts.iter().enumerate() {
        assert_eq!(
            count.get(),
            1,
            "row {i} is offscreen but a hero transition is in flight → it must paint"
        );
    }
    assert!(
        outcome.needs_frame,
        "the now-painting loops bubble their continuation request"
    );
    assert!(outcome.needs_frame_paced_only);
    assert_eq!(outcome.class, Some(TickClass::CosmeticLoop));
}

#[test]
fn culling_never_reorders_or_skips_children_still_within_the_warm_band() {
    // Sanity check that culling geometry is symmetric with
    // flex.rs's own unit tests: the boundary row exactly at the warm band's
    // bottom edge still paints (Rect::overlaps counts a touching edge as in).
    let (mut w, counts) = build_column(all_cosmetic());
    let vr = Rect::from_origin_size(Point::ZERO, Size::new(ROW_W, ROW_H));
    w.paint_at(Some(vr));
    assert_eq!(counts[2].get(), 1, "the y=200 boundary row stays warm");
}
