//! `PlatformViewSlot`: the app-facing leaf that reserves layout space for a
//! native platform view and publishes a
//! [`PlatformViewFrame`](frust_core::widget::PlatformViewFrame) every paint
//! (spec's platform-views feature, task 02).
//!
//! [`platform_view`] takes the `"dev.frust.<Factory>"`-style native factory
//! name and returns a builder ([`PlatformViewView`]) over the params/size
//! contract:
//!
//! ```ignore
//! platform_view("dev.frust.MapFactory")
//!     .params_json(r#"{"style":"dark"}"#)   // optional, default ""
//!     .size(300.0, 400.0)                   // explicit slot size; or:
//!     .expand()                             // fill given constraints
//! ```
//!
//! # Paint contract (Mode A / Mode B)
//!
//! This widget **paints nothing itself in v1**. Under Mode B (a translucent
//! frust surface composited over the native view) the transparent hole a
//! caller sees at the slot's bounds is simply the base surface's alpha-0
//! showing through — nobody paints here, by design. Under Mode A (an opaque
//! native sibling view placed on top of the frust surface) the native view
//! physically covers this region regardless of what frust paints underneath.
//! Either way, [`PlatformViewWidget::paint`] only ever *publishes* the frame
//! describing where/how the native view should be placed; the actual
//! composition happens downstream (task 03's differ, the per-shell channel
//! tasks). See `docs/CODE_STANDARDS.md`'s platform-view paint-contract entry
//! (task 12) for the full writeup.
//!
//! # Identity
//!
//! `slot_id` is allocated once per **widget instance**
//! ([`frust_core::widget::next_slot_id`], called from
//! [`View::build`](frust_core::View::build)) — never derived from tree
//! position, so a keyed reorder (spec §6.3) preserves it automatically along
//! with the rest of the retained widget. A `view_type` change across a
//! rebuild is treated like swapping in a widget of a different concrete type
//! (matching the framework's type-swap reconciliation conventions
//! elsewhere): a different native factory is a genuinely different slot, not
//! an update to the existing one, so it gets a fresh `slot_id` and its
//! `params_generation` resets to 0 as if newly built.
//!
//! # No input contract (v1)
//!
//! `PlatformViewWidget` has no [`Widget::event`](frust_core::Widget::event)
//! override — it relies on the trait's `Ignored` default. Mode A routes
//! input to the native sibling view at the OS level (frust never sees it);
//! Mode B has frust own the whole surface, but a platform-view slot's own
//! input contract is out of scope for v1 (see the feature's research notes).

use frust_core::accesskit::Role;
use frust_core::widget::{PlatformViewFrame, next_slot_id};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use kurbo::{Rect, Size};
#[cfg(debug_assertions)]
use peniko::Color;

/// How a [`PlatformViewSlot`] resolves its layout size.
///
/// `Expand` is the default (see [`platform_view`]): a native view most often
/// wants to fill whatever space its container gives it (a full-bleed map, a
/// video player), so an app that forgets to call either builder still gets a
/// sensible slot rather than a degenerate zero-size one. [`PlatformViewView::size`]
/// switches to `Explicit` for the common case of a fixed-size embed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SlotSize {
    /// Fill the incoming constraints' maximum.
    Expand,
    /// A fixed `(width, height)`, clamped into the incoming constraints.
    Explicit(f64, f64),
}

/// The translucent magenta a [`PlatformViewView::debug_fill`] slot paints —
/// a named constant (deliberately not theme-resolved: nothing here is
/// themed, see the module's Notes), invaluable on desktop where no native
/// host exists to show through the slot.
#[cfg(debug_assertions)]
const DEBUG_FILL_COLOR: Color = Color::from_rgba8(0xE0, 0x00, 0xE0, 0x40);

/// A declarative platform-view slot. See the [module docs](self).
pub struct PlatformViewView {
    view_type: String,
    params_json: String,
    slot_size: SlotSize,
    #[cfg(debug_assertions)]
    debug_fill: bool,
    semantics_label: Option<String>,
}

/// Reserve layout space for a native platform view created by `view_type`
/// (the `"dev.frust.<Factory>"` convention), filling its container by
/// default (see [`SlotSize::Expand`]). See the [module docs](self) for the
/// full builder contract.
pub fn platform_view(view_type: impl Into<String>) -> PlatformViewView {
    PlatformViewView {
        view_type: view_type.into(),
        params_json: String::new(),
        slot_size: SlotSize::Expand,
        #[cfg(debug_assertions)]
        debug_fill: false,
        semantics_label: None,
    }
}

impl PlatformViewView {
    /// Set the opaque creation params handed to the native factory (default
    /// empty). Changing this across a rebuild bumps the widget's retained
    /// `params_generation` counter exactly once (see the module's Identity
    /// notes) — the differ (task 03) uses the bump to decide whether to
    /// re-create or merely update the native view.
    pub fn params_json(mut self, params_json: impl Into<String>) -> Self {
        self.params_json = params_json.into();
        self
    }

    /// Force an explicit `(width, height)` slot size, clamped into whatever
    /// constraints this widget's parent hands it. Overrides
    /// [`PlatformViewView::expand`] when called after it (last builder call
    /// wins, matching the crate's other size-mode builders).
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.slot_size = SlotSize::Explicit(width, height);
        self
    }

    /// Fill the incoming constraints' maximum — the default (see
    /// [`SlotSize::Expand`]), provided as an explicit builder call for
    /// readability at call sites that want to make the choice visible.
    pub fn expand(mut self) -> Self {
        self.slot_size = SlotSize::Expand;
        self
    }

    /// Debug aid: paint a translucent magenta slab over the slot's bounds
    /// (see [`DEBUG_FILL_COLOR`]) so the reserved region is visible even
    /// with no native host attached — invaluable on desktop, where a
    /// platform view never actually composites. Compiled only into debug
    /// builds, so it can never ship live in a release binary.
    #[cfg(debug_assertions)]
    pub fn debug_fill(mut self) -> Self {
        self.debug_fill = true;
        self
    }

    /// Attach a plain accessible label over the slot's bounds (a
    /// [`Role::GenericContainer`] semantics node) — the v1 accessibility-gap
    /// mitigation: the native view itself carries no frust-visible
    /// semantics, so this is the only signal an assistive technology gets
    /// for the region.
    pub fn semantics_label(mut self, label: impl Into<String>) -> Self {
        self.semantics_label = Some(label.into());
        self
    }
}

/// The retained widget for a [`PlatformViewView`]. See the [module docs](self).
pub struct PlatformViewWidget {
    /// Stable per-widget-instance id (see the module's Identity notes) —
    /// allocated once at construction, never derived from tree position.
    slot_id: u64,
    view_type: String,
    params_json: String,
    /// Bumped whenever `params_json` changes across a rebuild (never on a
    /// `view_type` swap, which instead resets it to 0 — see the module's
    /// Identity notes).
    params_generation: u64,
    slot_size: SlotSize,
    #[cfg(debug_assertions)]
    debug_fill: bool,
    semantics_label: Option<String>,
}

impl<State: 'static> View<State> for PlatformViewView {
    type Element = PlatformViewWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PlatformViewWidget {
        PlatformViewWidget {
            slot_id: next_slot_id(),
            view_type: self.view_type.clone(),
            params_json: self.params_json.clone(),
            params_generation: 0,
            slot_size: self.slot_size,
            #[cfg(debug_assertions)]
            debug_fill: self.debug_fill,
            semantics_label: self.semantics_label.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PlatformViewWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if prev.view_type != self.view_type {
            // A different native factory is a different slot outright — treat
            // like a type swap (module docs' Identity section): fresh
            // slot_id, generation resets to 0 as if freshly built.
            element.slot_id = next_slot_id();
            element.view_type = self.view_type.clone();
            element.params_json = self.params_json.clone();
            element.params_generation = 0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if prev.params_json != self.params_json {
            element.params_json = self.params_json.clone();
            element.params_generation += 1;
            flags |= ChangeFlags::PAINT;
        }

        if prev.slot_size != self.slot_size {
            element.slot_size = self.slot_size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        #[cfg(debug_assertions)]
        if prev.debug_fill != self.debug_fill {
            element.debug_fill = self.debug_fill;
            flags |= ChangeFlags::PAINT;
        }

        if prev.semantics_label != self.semantics_label {
            // Semantics are recomputed each pass from the widget (mirroring
            // `IconView`'s label), so adopting the new value needs no
            // layout/paint dirtiness of its own.
            element.semantics_label = self.semantics_label.clone();
        }

        flags
    }
}

impl Widget for PlatformViewWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Nothing platform-y here — plain box-constraint arithmetic, exactly
        // like any other leaf.
        match self.slot_size {
            SlotSize::Explicit(width, height) => bc.constrain(Size::new(width, height)),
            SlotSize::Expand => bc.max(),
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let rect = Rect::from_origin_size(ctx.origin(), ctx.size());

        // `clip`/`visible`: intersect against a threaded scroll-ancestor
        // viewport when one exists; no constraint means fully visible (see
        // `PlatformViewFrame`'s doc comment). `overlaps` (not a zero-area
        // check on the intersection) is the same edge-inclusive test `Flex`'s
        // paint-time cull uses, so a slot that merely shares an edge with the
        // viewport still counts as visible.
        let (clip, visible) = match ctx.visible_rect() {
            Some(visible_rect) => (Some(visible_rect.intersect(rect)), visible_rect.overlaps(rect)),
            None => (None, true),
        };

        // Publish exactly once per paint — the whole point of this widget.
        ctx.publish_platform_view(PlatformViewFrame {
            slot_id: self.slot_id,
            view_type: self.view_type.clone(),
            params_json: self.params_json.clone(),
            params_generation: self.params_generation,
            rect,
            clip,
            visible,
        });

        // v1 paint contract: nothing else is painted here (see the module's
        // Paint contract section) except the debug-only fill aid.
        #[cfg(debug_assertions)]
        if self.debug_fill {
            scene.fill_rect(ctx.origin(), ctx.size(), DEBUG_FILL_COLOR);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(label) = &self.semantics_label {
            ctx.push_node(Role::GenericContainer, |node| {
                node.set_label(label.as_str());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Axis, EdgeInsets, FlexView, Padding, PaddingView, Row, SizedBox, keyed};
    use frust_core::{BuildCtx, ChildPod, any};
    use kurbo::Point;

    fn build(view: &PlatformViewView) -> PlatformViewWidget {
        let mut counter = 0u64;
        <PlatformViewView as View<()>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild(
        prev: &PlatformViewView,
        next: &PlatformViewView,
        element: &mut PlatformViewWidget,
    ) -> ChangeFlags {
        let mut counter = 0u64;
        <PlatformViewView as View<()>>::rebuild(next, prev, element, &mut BuildCtx::new(&mut counter))
    }

    /// A no-op recording scene — these tests only inspect the published
    /// `PlatformViewFrame`s, never any actual draw commands.
    #[derive(Default)]
    struct NullScene;

    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    // -- layout --------------------------------------------------------

    #[test]
    fn expand_fills_the_incoming_constraints_max() {
        let view = platform_view("dev.frust.MapFactory");
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, 200.0));
    }

    #[test]
    fn explicit_size_is_clamped_into_constraints() {
        let view = platform_view("dev.frust.MapFactory").size(1000.0, 1000.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, 200.0));
    }

    // -- absolute rect under nesting + scroll offset --------------------

    #[test]
    fn publishes_absolute_rect_under_padding_flex_nesting_and_scroll_offset() {
        let mut counter = 0u64;
        let insets = EdgeInsets::all(10.0);
        let row: FlexView<()> = Row(vec![
            any(SizedBox::<()>(Some(50.0), Some(30.0))),
            any(platform_view("dev.frust.MapFactory").size(80.0, 60.0)),
        ]);
        let padded: PaddingView<()> = Padding(insets, row);

        let widget = padded.build(&mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();

        // Wrap in a `ChildPod` and translate it by (-15, -25) — the same
        // shape `ScrollView` uses to offset scrolled content
        // (`self.child.set_origin(Point::new(0.0, -self.offset))`) —
        // standing in for "a simulated scroll offset".
        let mut pod = ChildPod::new(Box::new(widget));
        pod.set_origin(Point::new(-15.0, -25.0));
        pod.layout_child(&mut lctx, &BoxConstraints::loose(Size::new(1000.0, 1000.0)));

        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(2000.0, 2000.0));
        pod.paint_child(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        // Row places the SizedBox(50x30) first, so the platform_view sits at
        // main-axis offset 50; then the Padding's (10, 10) top-left inset;
        // then the (-15, -25) scroll translation.
        let expected_origin = Point::new(-15.0 + 10.0 + 50.0, -25.0 + 10.0);
        assert_eq!(
            frames[0].rect,
            Rect::from_origin_size(expected_origin, Size::new(80.0, 60.0))
        );
    }

    // -- clip / visibility ------------------------------------------------

    #[test]
    fn clip_is_intersected_with_visible_rect_when_straddling_edge() {
        let view = platform_view("dev.frust.MapFactory").size(100.0, 100.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));

        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::new(50.0, 0.0), Size::new(100.0, 100.0));
        pctx.constrain_visible_rect(Rect::from_origin_size(Point::ZERO, Size::new(100.0, 100.0)));
        w.paint(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        let frame: &PlatformViewFrame = &frames[0];
        assert_eq!(
            frame.rect,
            Rect::from_origin_size(Point::new(50.0, 0.0), Size::new(100.0, 100.0))
        );
        assert_eq!(frame.clip, Some(Rect::new(50.0, 0.0, 100.0, 100.0)));
        assert!(frame.visible, "an edge-straddling slot is still visible");
    }

    #[test]
    fn fully_outside_visible_rect_publishes_not_visible() {
        let view = platform_view("dev.frust.MapFactory").size(100.0, 100.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));

        let mut scene = NullScene;
        // Slot paints at (200, 0)-(300, 100); visible rect covers only
        // (0,0)-(100,100) — no overlap at all.
        let mut pctx = PaintCtx::new(Point::new(200.0, 0.0), Size::new(100.0, 100.0));
        pctx.constrain_visible_rect(Rect::from_origin_size(Point::ZERO, Size::new(100.0, 100.0)));
        w.paint(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].visible);
    }

    #[test]
    fn no_visible_rect_constraint_means_fully_visible_with_no_clip() {
        let view = platform_view("dev.frust.MapFactory").size(100.0, 100.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));

        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 100.0));
        w.paint(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert_eq!(frames[0].clip, None);
        assert!(frames[0].visible);
    }

    #[test]
    fn parent_culling_of_the_whole_subtree_yields_no_frame_at_all() {
        let mut counter = 0u64;
        let row: FlexView<()> = Row(vec![any(
            platform_view("dev.frust.MapFactory").size(50.0, 50.0),
        )]);
        let mut widget = row.build(&mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(1000.0, 1000.0)));

        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(1000.0, 1000.0));
        // Far outside even the one-viewport warm margin `Flex` extends the
        // cull test by, so the child never paints at all.
        pctx.constrain_visible_rect(Rect::from_origin_size(
            Point::new(5000.0, 5000.0),
            Size::new(10.0, 10.0),
        ));
        widget.paint(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert!(
            frames.is_empty(),
            "a culled subtree must publish no frame at all, not a hidden one"
        );
    }

    // -- params generation --------------------------------------------------

    #[test]
    fn params_json_change_bumps_generation_exactly_once_unchanged_does_not() {
        let prev = platform_view("dev.frust.MapFactory").params_json("a");
        let mut w = build(&prev);
        assert_eq!(w.params_generation, 0);

        let same = platform_view("dev.frust.MapFactory").params_json("a");
        rebuild(&prev, &same, &mut w);
        assert_eq!(w.params_generation, 0, "unchanged params must not bump");

        let changed = platform_view("dev.frust.MapFactory").params_json("b");
        rebuild(&same, &changed, &mut w);
        assert_eq!(w.params_generation, 1);

        let changed_again = platform_view("dev.frust.MapFactory").params_json("c");
        rebuild(&changed, &changed_again, &mut w);
        assert_eq!(w.params_generation, 2);
    }

    #[test]
    fn view_type_change_allocates_a_fresh_slot_id_and_resets_generation() {
        let prev = platform_view("dev.frust.MapFactory").params_json("a");
        let mut w = build(&prev);
        w.params_generation = 3; // pretend a couple of param bumps already happened
        let old_slot_id = w.slot_id;

        let swapped = platform_view("dev.frust.OtherFactory").params_json("a");
        rebuild(&prev, &swapped, &mut w);

        assert_ne!(w.slot_id, old_slot_id, "a view_type swap must allocate a new slot id");
        assert_eq!(w.params_generation, 0, "a swap resets generation like a fresh build");
        assert_eq!(w.view_type, "dev.frust.OtherFactory");
    }

    // -- keyed reorder identity ---------------------------------------------

    #[test]
    fn slot_id_survives_a_keyed_reorder() {
        // slot_id lives on the WIDGET (module docs' Identity notes), so the
        // existing keyed-reconciliation machinery preserves it across a
        // reorder with no platform-view-specific code needed — this proves
        // that by keying two slots, swapping their positions across a
        // rebuild, and checking each keeps its original slot_id.
        let mut counter = 0u64;
        let a: FlexView<()> = FlexView::new(
            Axis::Horizontal,
            vec![
                keyed(1u64, platform_view("dev.frust.A")),
                keyed(2u64, platform_view("dev.frust.B")),
            ],
        );
        let mut widget = a.build(&mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(1000.0, 1000.0)));

        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(1000.0, 1000.0));
        widget.paint(&mut pctx, &mut scene);
        let before = pctx.take_platform_views();
        assert_eq!(before.len(), 2);
        let (id_a, id_b) = (before[0].slot_id, before[1].slot_id);
        assert_ne!(id_a, id_b);

        // Rebuild with the two keyed children swapped in position.
        let b: FlexView<()> = FlexView::new(
            Axis::Horizontal,
            vec![
                keyed(2u64, platform_view("dev.frust.B")),
                keyed(1u64, platform_view("dev.frust.A")),
            ],
        );
        b.rebuild(&a, &mut widget, &mut BuildCtx::new(&mut counter));
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(1000.0, 1000.0)));

        let mut pctx2 = PaintCtx::new(Point::ZERO, Size::new(1000.0, 1000.0));
        widget.paint(&mut pctx2, &mut scene);
        let after = pctx2.take_platform_views();
        assert_eq!(after.len(), 2);
        // Position 0 is now key 2 ("B"), position 1 is key 1 ("A") — each
        // keeps its original slot_id despite the position swap.
        assert_eq!(after[0].slot_id, id_b, "key 2's slot survives the reorder");
        assert_eq!(after[1].slot_id, id_a, "key 1's slot survives the reorder");
    }
}
