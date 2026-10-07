//! Flex layout container: `Row`/`Column` over a main/cross axis.
//!
//! [`FlexView`]/[`FlexWidget`] are the declarative/retained pair (mirroring
//! [`crate::text`]'s `TextView`/`TextWidget`). A flex lays its children out along
//! a main [`Axis`], mirroring Flutter's `Flex`: inflexible children take their
//! natural main size first, then any remaining main-axis space is divided among
//! flexible children in proportion to their `flex` factor.
//!
//! Construct one with the [`Row`]/[`Column`] sugar (all children inflexible) or
//! [`FlexView::new`] with explicit [`FlexChild`]s built via [`flexible`] /
//! [`inflexible`] when some children should expand.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, ViewSeq, Widget, any,
};
use kurbo::{Point, Rect, Size};

use crate::ChildKey;

/// The axis a [`FlexView`] lays its children along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children are placed left-to-right; main = width, cross = height.
    Horizontal,
    /// Children are placed top-to-bottom; main = height, cross = width.
    Vertical,
}

impl Axis {
    /// The main-axis extent of `size`.
    fn main_of(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        }
    }

    /// The cross-axis extent of `size`.
    fn cross_of(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.height,
            Axis::Vertical => size.width,
        }
    }

    /// Build a [`Size`] from main/cross extents.
    fn size(self, main: f64, cross: f64) -> Size {
        match self {
            Axis::Horizontal => Size::new(main, cross),
            Axis::Vertical => Size::new(cross, main),
        }
    }

    /// Build a [`Point`] from main/cross coordinates.
    fn point(self, main: f64, cross: f64) -> Point {
        match self {
            Axis::Horizontal => Point::new(main, cross),
            Axis::Vertical => Point::new(cross, main),
        }
    }
}

/// How children are aligned along the cross axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossAxisAlignment {
    /// Pack each child at the cross-axis start (top for a Row, left for a Column).
    Start,
    /// Center each child on the cross axis.
    Center,
    /// Stretch each child to fill the cross axis (tight cross constraint).
    Stretch,
}

/// How children are distributed along the main axis.
///
/// v1 ships only [`MainAxisAlignment::Start`] (leading-packed); the
/// space-between/around/center variants are deferred to a later phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainAxisAlignment {
    /// Pack children at the main-axis start with no leading gap.
    Start,
}

/// One child of a [`FlexView`]: an erased child view, its `flex` factor
/// (`0` = inflexible; `> 0` = takes a proportional share of the free main space),
/// and an optional [`ChildKey`] for keyed reconciliation.
///
/// `key` is `None` for the plain [`Row`]/[`Column`]/[`flexible`]/[`inflexible`]
/// sugar (positional reconciliation, unchanged) and `Some` only for children
/// built with [`keyed`], which opts the whole list into key-matched
/// reconciliation so reorders/inserts preserve widget state.
pub struct FlexChild<State: 'static> {
    view: AnyView<State>,
    flex: u32,
    key: Option<ChildKey>,
}

/// A flexible child taking `flex` proportional shares of the free main-axis space.
pub fn flexible<State: 'static, V: View<State>>(flex: u32, view: V) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex,
        key: None,
    }
}

/// An inflexible child, sized to its natural main-axis extent.
pub fn inflexible<State: 'static, V: View<State>>(view: V) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex: 0,
        key: None,
    }
}

/// An inflexible child tagged with a stable [`ChildKey`], for a list whose items
/// reorder, insert, or delete between frames.
///
/// Attaching a key to *any* child opts the whole [`FlexView`] into keyed
/// reconciliation: on the next rebuild, children are matched to their live
/// widgets by key rather than by position, so a shuffled or grown list preserves
/// each surviving row's widget and its internal state (a scroll offset, a text
/// buffer, a toggle) instead of rebuilding whatever now sits at that index. Keys
/// are all-or-nothing per list and must be unique within it (see
/// [`ChildKey`]).
///
/// **All-or-nothing rule**: once one child has a key, *all* children in that
/// container must have keys — do not mix `.keyed(..)` with `.child(..)`/`.flex(..)`
/// in the same container. A keyed list cannot hold a `.flex` spacer. Mixing keyed
/// and unkeyed children debug-asserts on the first rebuild and falls back to
/// positional matching (preserving layout and interaction but losing the state
/// retention that keying adds).
///
/// Use it inside [`FlexView::new`] alongside (or instead of) [`inflexible`]:
///
/// ```
/// use frust_widgets::{Axis, FlexView, keyed, text};
/// # struct Item { id: u64, label: String }
/// # fn demo(items: &[Item]) -> FlexView<()> {
/// FlexView::new(
///     Axis::Vertical,
///     items.iter().map(|item| keyed(item.id, text(item.label.clone()))).collect(),
/// )
/// # }
/// ```
///
/// v1 keyed children are inflexible; combining a key with a `flex` factor is a
/// future extension.
pub fn keyed<State: 'static, V: View<State>>(
    key: impl Into<ChildKey>,
    view: V,
) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex: 0,
        key: Some(key.into()),
    }
}

/// A declarative flex container. See the [module docs](self).
pub struct FlexView<State: 'static> {
    direction: Axis,
    cross: CrossAxisAlignment,
    main: MainAxisAlignment,
    children: Vec<FlexChild<State>>,
}

impl<State: 'static> FlexView<State> {
    /// Create a flex laying `children` out along `direction`, cross-aligned to
    /// the start and main-aligned to the start.
    pub fn new(direction: Axis, children: Vec<FlexChild<State>>) -> Self {
        Self {
            direction,
            cross: CrossAxisAlignment::Start,
            main: MainAxisAlignment::Start,
            children,
        }
    }

    /// Append an inflexible child. Accepts any [`View`]; erasure happens
    /// internally, so no `any()` is needed.
    pub fn child<V: View<State>>(mut self, view: V) -> Self {
        self.children.push(inflexible(view));
        self
    }

    /// Append a flexible child taking `flex` proportional shares of the free
    /// main-axis space.
    pub fn flex<V: View<State>>(mut self, flex: u32, view: V) -> Self {
        self.children.push(flexible(flex, view));
        self
    }

    /// Append an inflexible child tagged with a stable [`ChildKey`] (see
    /// [`keyed`] for the reconciliation semantics and the all-or-nothing rule).
    pub fn keyed<V: View<State>>(mut self, key: impl Into<ChildKey>, view: V) -> Self {
        self.children.push(keyed(key, view));
        self
    }

    /// Append every element of `children` as an inflexible child.
    ///
    /// Accepts any [`ViewSeq`]: a `Vec`/array of views, a tuple of mixed view
    /// types, an `Option`, nested sequences, or `views(iter)` for an iterator.
    /// Tuples are positional: a `None` element drops its slot and nested
    /// sequences flatten in order. For a keyed list use [`FlexView::keyed`] per
    /// child (the all-or-nothing rule of [`keyed`] still applies; a tuple does
    /// not carry keys).
    pub fn children<M>(mut self, children: impl ViewSeq<State, M>) -> Self {
        let mut erased = Vec::new();
        children.extend_views(&mut erased);
        self.children
            .extend(erased.into_iter().map(|view| FlexChild {
                view,
                flex: 0,
                key: None,
            }));
        self
    }

    /// Append a pre-built [`FlexChild`] (escape hatch for [`flexible`] /
    /// [`inflexible`] / [`keyed`] values built elsewhere).
    pub fn push(mut self, child: FlexChild<State>) -> Self {
        self.children.push(child);
        self
    }

    /// Apply `f` to the builder only when `cond` is true.
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    /// Apply `f` with the contained value when `opt` is `Some`.
    pub fn when_some<T>(self, opt: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match opt {
            Some(value) => f(self, value),
            None => self,
        }
    }

    /// Set the cross-axis alignment.
    pub fn cross_axis(mut self, cross: CrossAxisAlignment) -> Self {
        self.cross = cross;
        self
    }

    /// Set the main-axis alignment.
    pub fn main_axis(mut self, main: MainAxisAlignment) -> Self {
        self.main = main;
        self
    }
}

/// An empty vertical flex, ready for fluent children: no `any()` needed.
///
/// ```
/// use frust_widgets::{FlexView, column, text};
/// # fn demo(cond: bool) -> FlexView<()> {
/// column()
///     .child(text("a"))
///     .flex(1, text("b"))
///     .when(cond, |c| c.child(text("c")))
/// # }
/// # let _ = demo(true);
/// ```
pub fn column<State: 'static>() -> FlexView<State> {
    FlexView::new(Axis::Vertical, Vec::new())
}

/// An empty horizontal flex, ready for fluent children. See [`column`].
pub fn row<State: 'static>() -> FlexView<State> {
    FlexView::new(Axis::Horizontal, Vec::new())
}

/// A horizontal flex (`Axis::Horizontal`) of inflexible children — the common
/// sugar. Use [`FlexView::new`] with [`flexible`] children when some should expand.
///
/// `children` is any [`ViewSeq`]: a `Vec`/array of views, a tuple of
/// mixed view types (`(a, b, c)`, positional, up to 12 and nestable), an
/// `Option` (`None` drops the slot), or `views(iter)` for an iterator. Every
/// element is erased once, here. A bare empty list needs its item type spelled
/// out (`Vec::<AnyView<State>>::new()`).
///
/// ```
/// use frust_core::any;
/// use frust_widgets::{FlexView, Row, text};
/// # fn demo() -> (FlexView<()>, FlexView<()>) {
/// let labels = Row(vec![text("a"), text("b"), text("c")]);
/// let mixed = Row(vec![any(text("a")), any(Row(vec![text("b")]))]);
/// # (labels, mixed)
/// # }
/// # let _ = demo();
/// ```
#[allow(non_snake_case)]
pub fn Row<State: 'static, M>(children: impl ViewSeq<State, M>) -> FlexView<State> {
    FlexView::new(Axis::Horizontal, Vec::new()).children(children)
}

/// A vertical flex (`Axis::Vertical`) of inflexible children — the common sugar.
/// Use [`FlexView::new`] with [`flexible`] children when some should expand.
///
/// `children` is any [`ViewSeq`]: a `Vec`/array of views, a tuple of
/// mixed view types (`(a, b, c)`, positional, up to 12 and nestable), an
/// `Option` (`None` drops the slot), or `views(iter)` for an iterator. Every
/// element is erased once, here. A bare empty list needs its item type spelled
/// out (`Vec::<AnyView<State>>::new()`).
///
/// ```
/// use frust_core::any;
/// use frust_widgets::{FlexView, Column, text};
/// # fn demo() -> (FlexView<()>, FlexView<()>) {
/// let labels = Column(vec![text("a"), text("b"), text("c")]);
/// let mixed = Column(vec![any(text("a")), any(Column(vec![text("b")]))]);
/// # (labels, mixed)
/// # }
/// # let _ = demo();
/// ```
#[allow(non_snake_case)]
pub fn Column<State: 'static, M>(children: impl ViewSeq<State, M>) -> FlexView<State> {
    FlexView::new(Axis::Vertical, Vec::new()).children(children)
}

/// The retained widget for a [`FlexView`]. Holds a parallel `children`/`flex`
/// pair (same length) so layout can index both without a per-child wrapper.
pub struct FlexWidget {
    direction: Axis,
    cross: CrossAxisAlignment,
    main: MainAxisAlignment,
    children: Vec<ChildPod>,
    flex: Vec<u32>,
}

/// Build the box constraints for one flex child.
///
/// `main_min..main_max` bound the main axis (inflexible children get
/// `0..∞`; flexible children a tight `share..share`). Under `stretch` the cross
/// axis is tight at `cross_bound`; otherwise it is loose up to `cross_max`.
fn child_constraints(
    axis: Axis,
    main_min: f64,
    main_max: f64,
    cross_bound: f64,
    stretch: bool,
    cross_max: f64,
) -> BoxConstraints {
    let cross_min = if stretch { cross_bound } else { 0.0 };
    let cross_hi = if stretch { cross_bound } else { cross_max };
    BoxConstraints::new(
        axis.size(main_min, cross_min),
        axis.size(main_max, cross_hi),
    )
}

impl<State: 'static> View<State> for FlexView<State> {
    type Element = FlexWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FlexWidget {
        let mut children = Vec::with_capacity(self.children.len());
        let mut flex = Vec::with_capacity(self.children.len());
        for child in &self.children {
            children.push(crate::authoring::build_child(&child.view, ctx));
            flex.push(child.flex);
        }
        FlexWidget {
            direction: self.direction,
            cross: self.cross,
            main: self.main,
            children,
            flex,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FlexWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.direction != self.direction || prev.cross != self.cross || prev.main != self.main {
            element.direction = self.direction;
            element.cross = self.cross;
            element.main = self.main;
            flags |= ChangeFlags::LAYOUT;
        }

        // Reconcile the child pods through the shared helper (build/rebuild/
        // teardown + focus/capture retention for unchanged siblings across a
        // structural change). Keyed children (`|child| child.key`) opt the list
        // into key-matched reconciliation; an all-unkeyed list stays positional.
        flags |= crate::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |child| &child.view,
            |child| child.key,
        );

        // Rebuild the parallel `flex` sidecar to match the reconciled children's
        // new order and length in one shot — the keyed path may have reordered
        // them, so an index-wise diff no longer tracks a given child. Comparing
        // against the previous sidecar keeps the layout-dirty signal a factor
        // change (or length/order change) still deserves.
        let new_flex: Vec<u32> = self.children.iter().map(|child| child.flex).collect();
        if new_flex != element.flex {
            element.flex = new_flex;
            flags |= ChangeFlags::LAYOUT;
        }

        flags
    }

    fn teardown(&self, element: &mut FlexWidget, ctx: &mut BuildCtx<'_>) {
        for (child, pod) in self.children.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(&child.view, pod, ctx);
        }
    }
}

impl Widget for FlexWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let axis = self.direction;
        let max_main = axis.main_of(bc.max());
        let max_cross = axis.cross_of(bc.max());
        let stretch = self.cross == CrossAxisAlignment::Stretch;
        // Stretch needs a finite cross bound to stretch to; fall back to zero
        // under an (unusual) unbounded cross constraint.
        let cross_bound = if max_cross.is_finite() {
            max_cross
        } else {
            0.0
        };

        // Pass 1: lay out inflexible children under an unbounded main axis,
        // accumulating the space they consume and the total flex weight.
        let mut allocated_main = 0.0f64;
        let mut total_flex = 0u32;
        let mut max_child_cross = 0.0f64;
        for (i, pod) in self.children.iter_mut().enumerate() {
            if self.flex[i] == 0 {
                let cbc =
                    child_constraints(axis, 0.0, f64::INFINITY, cross_bound, stretch, max_cross);
                let size = pod.layout_child(ctx, &cbc);
                allocated_main += axis.main_of(size);
                max_child_cross = max_child_cross.max(axis.cross_of(size));
            } else {
                total_flex += self.flex[i];
            }
        }

        // Pass 2: divide the remaining main-axis space among flexible children in
        // proportion to their flex factor, laying each out under a tight main
        // constraint equal to its share.
        let free = if max_main.is_finite() {
            (max_main - allocated_main).max(0.0)
        } else {
            0.0
        };
        if total_flex > 0 {
            for (i, pod) in self.children.iter_mut().enumerate() {
                if self.flex[i] > 0 {
                    let share = free * (self.flex[i] as f64) / (total_flex as f64);
                    let cbc =
                        child_constraints(axis, share, share, cross_bound, stretch, max_cross);
                    let size = pod.layout_child(ctx, &cbc);
                    max_child_cross = max_child_cross.max(axis.cross_of(size));
                }
            }
        }

        // Main extent fills the constraint when flexible children are present (and
        // bounded); otherwise it shrink-wraps to the sum of the children.
        let main_size = if total_flex > 0 && max_main.is_finite() {
            max_main
        } else {
            allocated_main
        };
        // Cross extent fills under stretch, else shrink-wraps to the widest child.
        let cross_size = if stretch && max_cross.is_finite() {
            max_cross
        } else {
            max_child_cross
        };

        // Position children sequentially along the main axis (MainAxisAlignment
        // v1 = Start → no leading gap), cross-aligned per CrossAxisAlignment.
        let leading = match self.main {
            MainAxisAlignment::Start => 0.0,
        };
        let mut main_pos = leading;
        for pod in &mut self.children {
            let child_cross = axis.cross_of(pod.size());
            let cross_pos = match self.cross {
                CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => 0.0,
                CrossAxisAlignment::Center => (cross_size - child_cross) / 2.0,
            };
            pod.set_origin(axis.point(main_pos, cross_pos));
            main_pos += axis.main_of(pod.size());
        }

        bc.constrain(axis.size(main_size, cross_size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Paint-time visible-rect culling: when a scroll ancestor has threaded a
        // visible rect (see `PaintCtx::constrain_visible_rect`), skip painting any
        // child whose absolute bounds fall fully outside it plus a one-viewport
        // warm margin — so an offscreen animator below the fold never bubbles its
        // `request_frame` (paint is where that happens), and near-edge content
        // stays warm for a small scroll. `None` = no constraint → paint every
        // child, unchanged. Culling is paint-only: layout, events, capture, and
        // focus all route by layout geometry and are untouched.
        //
        // The cull tests a child's LAYOUT BOX ONLY — paint-time transforms
        // (`AnimatedScale`'s `push_transform`) are deliberately not consulted, so
        // the decision stays cheap and needs no per-child paint probe. This is
        // sound on two grounds: (a) the one-viewport warm margin (`vr.inflate` by
        // a full width/height each side) dwarfs any realistic transform overflow
        // — the catalog's largest scaled-glow instance
        // (`examples/glyph-catalog/src/pages/interactions.rs`'s `demo_charge_ring`,
        // an `AnimatedScale(1.03, …)` over a ~366px-wide row) overflows its layout
        // box by only ~11px on the dominant width axis (0.03 × 366), hundreds of
        // px inside the margin; and (b) a known overflower makes its layout box
        // reflect its max visual extent via the headroom-slot pattern (the
        // `HB_RING_SLOT` precedent in that same file). See the pre-existing
        // `AnimatedScale`/`Flex` sibling-layout defect noted at
        // `interactions.rs`'s `demo_charge_ring` (the `HB_RING_SCALE_MIN`
        // workaround comment) — a separate, layout-time interaction, cross-
        // referenced here because it is the other place transform-vs-Flex-box
        // divergence bites.
        //
        // Two exemptions relax the cull (they only ever ADD paints, never remove
        // one, so offscreen-ANIMATOR suppression is preserved for every other
        // child — a focused/hero child bypasses it by design):
        //  1. A FOCUSED child (`pod.is_focused()`, at most one per Flex) always
        //     paints. Its paint-time `publish_ime_state` is the ONLY resync
        //     channel for a rebuild-driven (non-event) controlled change to a
        //     focused field; culling it beyond the warm band would strand a stale
        //     IME surface until the field re-entered the viewport.
        //  2. While a hero transition is in flight (`ctx.hero_active()`), NO child
        //     is culled — a tagged descendant scrolled past the warm band must
        //     still paint so it reports its rest bounds (`report_hero`) for the
        //     morph. This is the widest-net form (any child, not just the tagged
        //     one): Flex cannot cheaply identify which child carries a hero tag
        //     from its paint context, and the exemption only applies during the
        //     brief transition, so the extra paints are bounded.
        let cull = ctx
            .visible_rect()
            .map(|vr| vr.inflate(vr.width(), vr.height()));
        let hero_in_flight = ctx.hero_active();
        // Paint in child order (first child painted first / bottom-most).
        for pod in &mut self.children {
            if let Some(warm) = cull {
                let exempt = hero_in_flight || pod.is_focused();
                if !exempt {
                    let child_abs =
                        Rect::from_origin_size(ctx.origin() + pod.origin().to_vec2(), pod.size());
                    if !child_abs.overlaps(warm) {
                        continue;
                    }
                }
            }
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Hit-test in reverse paint order (topmost/last-painted child first).
        crate::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A transparent layout container: contribute no node of its own, just
        // forward each child so their nodes attach to the enclosing node.
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    crate::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf};

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build<S: 'static>(view: &FlexView<S>) -> FlexWidget {
        let mut counter = 0u64;
        view.build(&mut ctx(&mut counter))
    }

    #[test]
    fn distributes_free_space_by_flex_factors() {
        // Two flexible children, factors 2:1, under a 300px-wide bound.
        // free = 300 (no inflexible children) → shares 200 and 100.
        let view: FlexView<()> = FlexView::new(
            Axis::Horizontal,
            vec![
                flexible(2, leaf(1000.0, 20.0)),
                flexible(1, leaf(1000.0, 20.0)),
            ],
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        assert_eq!(w.children[0].size().width, 200.0);
        assert_eq!(w.children[1].size().width, 100.0);
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 200.0);
        // Flexible children present → main axis fills the 300px bound.
        assert_eq!(size.width, 300.0);
    }

    #[test]
    fn mixes_inflexible_and_flexible() {
        // One inflexible 50px child, one flexible child, under 200px.
        // free = 200 - 50 = 150 → the flexible child takes all 150.
        let view: FlexView<()> = FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(leaf(50.0, 20.0)),
                flexible(1, leaf(1000.0, 20.0)),
            ],
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));

        assert_eq!(w.children[0].size().width, 50.0);
        assert_eq!(w.children[1].size().width, 150.0);
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 50.0);
        assert_eq!(size.width, 200.0);
    }

    #[test]
    fn shrink_wraps_main_axis_without_flexible_children() {
        // No flexible children → main extent is the sum of child widths (60),
        // not the 300px bound.
        let view: FlexView<()> = row()
            .child(leaf(40.0, 10.0).into_any())
            .child(leaf(20.0, 10.0).into_any());
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        assert_eq!(size.width, 60.0);
        assert_eq!(w.children[1].origin().x, 40.0);
    }

    #[test]
    fn cross_axis_stretch_tightens_children() {
        // Stretch → every child gets a tight cross constraint = the 80px bound,
        // overriding its 10px intrinsic height.
        let view: FlexView<()> = row()
            .child(leaf(30.0, 10.0).into_any())
            .cross_axis(CrossAxisAlignment::Stretch);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 80.0)));
        assert_eq!(w.children[0].size().height, 80.0);
        assert_eq!(size.height, 80.0);
    }

    #[test]
    fn cross_axis_start_packs_at_zero() {
        // Start → the shorter child sits at cross 0. cross_size = tallest = 40.
        let view: FlexView<()> = row()
            .child(leaf(10.0, 40.0).into_any())
            .child(leaf(10.0, 20.0).into_any())
            .cross_axis(CrossAxisAlignment::Start);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(size.height, 40.0);
        assert_eq!(w.children[1].origin().y, 0.0);
    }

    #[test]
    fn cross_axis_center_centers_shorter_children() {
        // Center → cross_size = 40; the 20px-tall child is centered at (40-20)/2.
        let view: FlexView<()> = row()
            .child(leaf(10.0, 40.0).into_any())
            .child(leaf(10.0, 20.0).into_any())
            .cross_axis(CrossAxisAlignment::Center);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(w.children[1].origin().y, 10.0);
    }

    #[test]
    fn column_lays_out_along_vertical_axis() {
        // A Column stacks children top-to-bottom: main = height.
        let view: FlexView<()> = column()
            .child(leaf(30.0, 15.0).into_any())
            .child(leaf(30.0, 25.0).into_any());
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 300.0)));
        assert_eq!(size.height, 40.0); // 15 + 25
        assert_eq!(w.children[0].origin().y, 0.0);
        assert_eq!(w.children[1].origin().y, 15.0);
    }

    #[test]
    fn paints_children_in_order() {
        let view: FlexView<()> = row()
            .child(leaf(20.0, 20.0).into_any())
            .child(leaf(20.0, 20.0).into_any());
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));

        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 200.0));
        w.paint(&mut pctx, &mut scene);
        // Child 0 painted at x=0, child 1 at x=20 — in child order.
        assert_eq!(scene.rects[0].0, Point::new(0.0, 0.0));
        assert_eq!(scene.rects[1].0, Point::new(20.0, 0.0));
    }

    // --- Paint-time visible-rect culling ------------------------------------

    /// Build+lay out a 5-row vertical column of 100x100 leaves (rows at
    /// y = 0,100,200,300,400) inside a 100x500 box.
    fn culling_column() -> FlexWidget {
        let view: FlexView<()> = column()
            .child(leaf(100.0, 100.0).into_any())
            .child(leaf(100.0, 100.0).into_any())
            .child(leaf(100.0, 100.0).into_any())
            .child(leaf(100.0, 100.0).into_any())
            .child(leaf(100.0, 100.0).into_any());
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 500.0)));
        w
    }

    #[test]
    fn no_visible_rect_paints_every_child() {
        // Default (no threaded visible rect) = paint everything, unchanged.
        let mut w = culling_column();
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 500.0));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects.len(), 5, "no cull → all five rows painted");
    }

    #[test]
    fn culls_children_fully_outside_visible_rect_plus_margin() {
        // Visible rect = the top 100px viewport at the origin. The warm margin is
        // one viewport (100px) on each side, so the warm band is y ∈ [-100, 200].
        // Rows at y=0/100/200 overlap it (the y=200 row touches the bottom edge,
        // which `Rect::overlaps` counts as in); rows at y=300/400 are fully outside
        // and culled.
        let mut w = culling_column();
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 500.0));
        pctx.constrain_visible_rect(Rect::from_origin_size(Point::ZERO, Size::new(100.0, 100.0)));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects.len(), 3, "two below-the-warm-band rows culled");
        assert_eq!(scene.rects[0].0, Point::new(0.0, 0.0));
        assert_eq!(scene.rects[1].0, Point::new(0.0, 100.0));
        // The boundary row at the warm-band's exact bottom edge stays warm.
        assert_eq!(scene.rects[2].0, Point::new(0.0, 200.0));
    }

    #[test]
    fn margin_boundary_row_just_past_the_warm_band_is_culled() {
        // A visible rect one pixel short of the y=200 row's top makes the warm band
        // y ∈ [-99, 201]... instead pick a rect whose inflated band excludes row 3
        // (y=300) but includes row 2 (y=200): rect height 50 at origin → warm band
        // y ∈ [-50, 100]. Row 0 (0..100) and row 1 (100..200 → touches 100) stay;
        // rows 2/3/4 are culled.
        let mut w = culling_column();
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 500.0));
        pctx.constrain_visible_rect(Rect::from_origin_size(Point::ZERO, Size::new(100.0, 50.0)));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects.len(), 2, "only the top band rows survive");
        assert_eq!(scene.rects[0].0, Point::new(0.0, 0.0));
        assert_eq!(scene.rects[1].0, Point::new(0.0, 100.0));
    }

    #[test]
    fn culled_child_still_receives_events_at_its_layout_geometry() {
        // Culling is paint-only: events route by layout geometry. Eight capturing
        // rows (ROW_H each); paint with a tiny visible rect that culls the lower
        // rows, then prove a tap at a culled row's geometry still captures and
        // fires on up-inside.
        let mut counter = 0u64;
        let view: FlexView<Vec<u32>> = column()
            .child(captor(0))
            .child(captor(1))
            .child(captor(2))
            .child(captor(3))
            .child(captor(4))
            .child(captor(5))
            .child(captor(6))
            .child(captor(7));
        let mut w = view.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        // Warm band = the top row inflated by one ROW_H each side → y ∈ [-20, 40];
        // row 7 (y 140..160) is far outside and culled from paint.
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * 8.0));
        pctx.constrain_visible_rect(Rect::from_origin_size(Point::ZERO, Size::new(ROW_W, ROW_H)));
        w.paint(&mut pctx, &mut scene);

        // A Down at row 7's midpoint still captures despite it being culled, and
        // the release fires it — event routing is untouched by paint culling.
        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(7)));
        assert!(
            w.children[7].is_active(),
            "a culled row still captures on Down"
        );
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(7)));
        assert_eq!(log, vec![7], "a culled-but-laid-out row still fires on Up");
    }

    // --- Cull exemptions: focused child + hero-in-flight --------------------
    //
    // Both exemptions only ever ADD a paint: an offscreen ANIMATOR with neither
    // property is still suppressed (the whole point of visible-rect culling),
    // which the "unfocused sibling still culled" assertions below keep honest
    // — a focused/hero child bypasses that suppression by design.

    /// A leaf that, on every paint, bumps a shared paint counter, fills a rect,
    /// and republishes an [`ImeState`] carrying its current `value` — standing in
    /// for `TextInput`'s paint-time `publish_ime_state`, the only resync channel
    /// for a rebuild-driven controlled change to a focused field.
    struct ImeLeaf {
        value: String,
        painted: Rc<Cell<u32>>,
    }
    /// Retained widget for [`ImeLeaf`].
    struct ImeLeafWidget {
        value: String,
        painted: Rc<Cell<u32>>,
    }

    impl View<()> for ImeLeaf {
        type Element = ImeLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeLeafWidget {
            ImeLeafWidget {
                value: self.value.clone(),
                painted: self.painted.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ImeLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // A controlled change threaded in via rebuild (never an event) — an
            // app-driven IME state update takes exactly this shape.
            element.value = self.value.clone();
            ChangeFlags::NONE
        }
    }

    impl Widget for ImeLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            use frust_core::{EditingState, ImeState};
            self.painted.set(self.painted.get() + 1);
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
            ctx.publish_ime_state(ImeState {
                active: true,
                editing: EditingState {
                    text: self.value.clone(),
                    selection_base: -1,
                    selection_extent: -1,
                    composing_base: -1,
                    composing_extent: -1,
                },
                caret: None,
                content_type: Default::default(),
                suppress_soft_keyboard: false,
            });
        }
    }

    /// Build a 5-row vertical `Column` of [`ImeLeaf`]s (row `i` value `"row{i}"`),
    /// returning the laid-out widget plus one paint counter per row.
    fn ime_column() -> (FlexWidget, [Rc<Cell<u32>>; 5]) {
        let counts: [Rc<Cell<u32>>; 5] = std::array::from_fn(|_| Rc::new(Cell::new(0)));
        let view: FlexView<()> = Column(
            (0..5)
                .map(|i| {
                    any(ImeLeaf {
                        value: format!("row{i}"),
                        painted: counts[i].clone(),
                    })
                })
                .collect::<Vec<_>>(),
        );
        let mut counter = 0u64;
        let mut w = view.build(&mut ctx(&mut counter));
        layout_column(&mut w);
        (w, counts)
    }

    /// A top-row visible rect: warm band = y ∈ [-ROW_H, 2·ROW_H] → rows 0/1/2
    /// stay, rows 3/4 fall outside. Mirrors the culling tests' geometry.
    fn top_row_rect() -> Rect {
        Rect::from_origin_size(Point::ZERO, Size::new(ROW_W, ROW_H))
    }

    #[test]
    fn focused_child_beyond_warm_band_still_paints_and_republishes_ime() {
        // Row 4 (y 80..100) is fully outside the warm band but FOCUSED, so it must
        // still paint and its IME republish must reach the container's PaintCtx.
        // Row 3 (also outside) is unfocused → still culled: suppression intact.
        let (mut w, counts) = ime_column();
        w.children[4].set_focused(true);

        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * 5.0));
        pctx.constrain_visible_rect(top_row_rect());
        w.paint(&mut pctx, &mut scene);

        assert_eq!(counts[0].get(), 1, "warm row 0 paints");
        assert_eq!(counts[1].get(), 1, "warm row 1 paints");
        assert_eq!(counts[2].get(), 1, "warm boundary row 2 paints");
        assert_eq!(
            counts[3].get(),
            0,
            "unfocused offscreen row 3 stays culled (suppression intact)"
        );
        assert_eq!(
            counts[4].get(),
            1,
            "focused offscreen row 4 is exempt from culling and paints"
        );
        // The focused row painted last, so its republished IME surface is the one
        // that bubbled up — proving the resync channel is reachable while culled.
        let ime = pctx
            .take_ime_state()
            .expect("focused row republished its IME");
        assert_eq!(ime.editing.text, "row4");
    }

    #[test]
    fn focused_cull_exemption_republishes_a_rebuild_mutation_immediately() {
        // The stale-then-fixed regression. An offscreen field
        // whose value is mutated via REBUILD (no event) must republish on the very
        // next paint. Unfocused: culled → no republish → the shell keeps a stale
        // surface (the bug). Focused: exempt → republished immediately (the fix).
        let far = Rect::from_origin_size(Point::new(0.0, 10_000.0), Size::new(ROW_W, ROW_H));

        // --- Stale case: the offscreen row is NOT focused. ---
        let mut counter = 0u64;
        let prev: FlexView<()> = column().child(ImeLeaf {
            value: "v1".to_string(),
            painted: Rc::new(Cell::new(0)),
        });
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);
        // Controlled change via rebuild → "v2", but the row is offscreen+unfocused.
        let next: FlexView<()> = column().child(ImeLeaf {
            value: "v2".to_string(),
            painted: Rc::new(Cell::new(0)),
        });
        next.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * 5.0));
        pctx.constrain_visible_rect(far);
        w.paint(&mut pctx, &mut scene);
        assert!(
            pctx.take_ime_state().is_none(),
            "an unfocused, culled field never republishes — its IME goes stale"
        );

        // --- Fixed case: the same offscreen row, now FOCUSED. ---
        let mut counter = 0u64;
        let prev: FlexView<()> = column().child(ImeLeaf {
            value: "v1".to_string(),
            painted: Rc::new(Cell::new(0)),
        });
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);
        w.children[0].set_focused(true);
        let next: FlexView<()> = column().child(ImeLeaf {
            value: "v2".to_string(),
            painted: Rc::new(Cell::new(0)),
        });
        next.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * 5.0));
        pctx.constrain_visible_rect(far);
        w.paint(&mut pctx, &mut scene);
        let ime = pctx
            .take_ime_state()
            .expect("focused field republishes even while offscreen");
        assert_eq!(
            ime.editing.text, "v2",
            "the rebuild-mutated value republished immediately, not stale 'v1'"
        );
    }

    #[test]
    fn animated_scale_child_straddling_the_cull_boundary_does_not_pop() {
        // The cull is LAYOUT-BOX-ONLY. An AnimatedScale child
        // magnifies its paint far past its layout box (~2.8×), but the cull tests
        // the box, so a child whose BOX overlaps the warm band paints regardless
        // of scale (no scale-driven pop), and one whose box is fully outside is
        // still culled (its transform overflow is dwarfed by the one-viewport
        // margin — the safe trade the contract comment documents). Scale is driven
        // directly via a zero-duration timing that snaps on the first paint (no
        // wall-clock).
        use frust_core::Curve;
        use std::time::Duration;
        let snap = crate::Timing::Duration(Duration::ZERO, Curve::Linear);

        // Rows 0..4 at y = i·ROW_H. Row 2 (y 40..60) sits on the warm-band bottom
        // edge (band = [-20, 40]) → box overlaps; row 4 (y 80..100) is fully out.
        let view: FlexView<()> = column()
            .child(leaf(ROW_W, ROW_H).into_any())
            .child(leaf(ROW_W, ROW_H).into_any())
            .child(crate::motion::AnimatedScale(2.8, leaf(ROW_W, ROW_H)).timing(snap))
            .child(leaf(ROW_W, ROW_H).into_any())
            .child(crate::motion::AnimatedScale(2.8, leaf(ROW_W, ROW_H)).timing(snap));
        let mut counter = 0u64;
        let mut w = view.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(ROW_W, ROW_H * 5.0));
        pctx.constrain_visible_rect(top_row_rect());
        w.paint(&mut pctx, &mut scene);

        // Rows 0,1 (plain) + row 2 (AnimatedScale, box touches the band) painted →
        // 3 fills; row 3 (plain) and row 4 (AnimatedScale) are outside → culled.
        assert_eq!(
            scene.rects.len(),
            3,
            "the boundary AnimatedScale row paints on its layout box, the two \
             fully-outside rows (one of them also AnimatedScale) are culled"
        );
        // The boundary AnimatedScale actually composited at ~2.8× (a transform was
        // pushed) — proving the magnified child painted, not a hairline stand-in.
        assert!(
            scene.transforms.iter().any(|t| {
                let c = t.as_coeffs();
                (c[0] - 2.8).abs() < 1e-6 && (c[3] - 2.8).abs() < 1e-6
            }),
            "the boundary row composited at 2.8× without being culled by its \
             transform-overflowed visual bounds"
        );
    }

    #[test]
    fn children_vec_diff_adds_removes_and_type_swaps() {
        // Start with two Leaf children.
        let mut counter = 0u64;
        let prev: FlexView<()> = row()
            .child(leaf(10.0, 10.0).into_any())
            .child(leaf(10.0, 10.0).into_any());
        let mut w = prev.build(&mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);

        // Grow to three.
        let grown: FlexView<()> = row()
            .child(leaf(10.0, 10.0).into_any())
            .child(leaf(10.0, 10.0).into_any())
            .child(leaf(10.0, 10.0).into_any());
        let flags = grown.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 3);
        assert!(flags.needs_layout());

        // Shrink to one.
        let shrunk: FlexView<()> = row().child(leaf(10.0, 10.0).into_any());
        shrunk.rebuild(&grown, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 1);
        assert_eq!(w.flex.len(), 1);

        // Type-swap the sole child (Leaf → the other test widget via AnyView).
        let swapped: FlexView<()> = row().child(crate::test_support::swap_leaf().into_any());
        swapped.rebuild(&shrunk, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 1);
        // The swapped widget reports a distinctive size, proving the swap took.
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(w.children[0].size(), Size::new(7.0, 7.0));
    }

    // --- Capture-vs-rebuild fixtures ----------------------------------------
    //
    // A vertical list of fixed 50x20 rows, each of which captures on `Down` and
    // "fires" (records its id into the `Vec<u32>` app state) only on an `Up`
    // while still armed. `Cancel` disarms WITHOUT touching app state — which is
    // what makes the rebuild-path synthetic cancel (driven over a `()` dummy
    // state) sound; a Cancel arm that read state would panic on the `()`
    // downcast, so these tests also guard that contract.

    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    use frust_core::{
        EventCtx, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, PointerPhase, any,
    };

    const ROW_W: f64 = 50.0;
    const ROW_H: f64 = 20.0;

    /// A row that captures on `Down` and fires its id on up-inside.
    struct Captor {
        id: u32,
    }
    /// Retained widget for [`Captor`].
    struct CaptorWidget {
        id: u32,
        armed: bool,
    }

    /// Erase a [`Captor`] tagged `id` into an `AnyView<Vec<u32>>`.
    fn captor(id: u32) -> impl View<Vec<u32>> {
        Captor { id }
    }

    impl View<Vec<u32>> for Captor {
        type Element = CaptorWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CaptorWidget {
            CaptorWidget {
                id: self.id,
                armed: false,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CaptorWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.id = self.id;
            ChangeFlags::NONE
        }
    }

    impl Widget for CaptorWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            match p.phase {
                PointerPhase::Down => {
                    self.armed = true;
                    ctx.capture_pointer();
                    EventResult::Handled
                }
                PointerPhase::Move => EventResult::Handled,
                PointerPhase::Up => {
                    if self.armed {
                        ctx.state_mut::<Vec<u32>>().push(self.id);
                    }
                    self.armed = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // Clears armed WITHOUT reading app state (g2 contract).
                    self.armed = false;
                    EventResult::Handled
                }
            }
        }
    }

    /// A row that records into a shared cell that it saw *any* event — used to
    /// prove a freshly type-swapped widget receives nothing until a new `Down`.
    struct Recorder {
        seen: Rc<Cell<u32>>,
    }
    /// Retained widget for [`Recorder`].
    struct RecorderWidget {
        seen: Rc<Cell<u32>>,
    }

    impl View<Vec<u32>> for Recorder {
        type Element = RecorderWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RecorderWidget {
            RecorderWidget {
                seen: self.seen.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut RecorderWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for RecorderWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            self.seen.set(self.seen.get() + 1);
            EventResult::Handled
        }
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// Dispatch one event to the flex over a `Vec<u32>` fire-log state.
    ///
    /// The log stays a `Vec<u32>` (not a slice) because it is erased as
    /// `&mut dyn Any` and recovered by the widgets via `state_mut::<Vec<u32>>()`.
    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut FlexWidget, log: &mut Vec<u32>, event: &InputEvent) {
        let state: &mut dyn Any = log;
        let mut ectx = EventCtx::new(state, Point::ZERO, Size::new(ROW_W, ROW_H * 8.0));
        w.event(&mut ectx, event);
    }

    /// Lay a Captor/Recorder column out so rows sit at y = i * ROW_H.
    fn layout_column(w: &mut FlexWidget) {
        let mut lctx = LayoutCtx::new();
        w.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(ROW_W, ROW_H * 8.0)),
        );
    }

    /// Y within row `i` (its vertical midpoint).
    fn row_y(i: usize) -> f64 {
        i as f64 * ROW_H + ROW_H / 2.0
    }

    #[test]
    fn append_after_preserves_captured_drag_before_change() {
        // An armed child BEFORE the change point survives
        // an append-after: the appended tail is past the stable prefix, so the
        // captured row keeps its `active` path and fires on Up as normal. This is
        // Flutter's invariant — a sibling structural change must not break an
        // unchanged child's in-flight gesture.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column().child(captor(0)).child(captor(1)).child(captor(2));
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active(), "row 1 captured the pointer");

        // Append a new row AFTER the captured one → length grows, no type swap.
        let appended: FlexView<Vec<u32>> = column()
            .child(captor(0))
            .child(captor(1))
            .child(captor(2))
            .child(captor(3));
        appended.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_active(),
            "append-after preserves the captured row's active path (stable prefix)"
        );

        // The captured drag completes and fires on the still-armed row.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert_eq!(log, vec![1], "captured row fires on Up as normal");
    }

    #[test]
    fn type_swap_before_armed_index_cancels_with_synthetic_cancel() {
        // An armed child at an index PAST the change point
        // (a type swap at an earlier index drops the stable prefix to that swap, so
        // the armed row sits in the cancelled tail) still receives a synthetic
        // `Cancel` — it unwinds its state machine rather than being silently
        // dropped or firing on a later hit-tested `Up`.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column().child(captor(0)).child(captor(1)).child(captor(2));
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(2)));
        assert!(w.children[2].is_active(), "row 2 captured the pointer");

        // Swap row 0 (before the armed index) to a different concrete type → the
        // stable prefix ends at index 0, so the armed row 2 is in the cancelled
        // tail. A CaptorWidget that received `Cancel` disarms (its Cancel arm sets
        // `armed = false`); one that never received it would still fire on Up.
        let seen = Rc::new(Cell::new(0u32));
        let swapped: FlexView<Vec<u32>> = column()
            .child(Recorder { seen })
            .child(captor(1))
            .child(captor(2));
        swapped.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children.iter().all(|p| !p.is_active()),
            "swap before the armed index cancelled the tail's active path"
        );

        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(2)));
        assert!(
            log.is_empty(),
            "no fire on Up — the armed row was synthetically cancelled"
        );
    }

    #[test]
    fn structural_truncation_of_active_row_unwinds_without_panic() {
        // (Scenario 2) Drag armed in row 2; a rebuild truncates the list to two
        // rows, dropping the active row. teardown_child cancels it: no panic, no
        // fire.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column()
            .child(captor(0))
            .child(captor(1))
            .child(captor(2))
            .child(captor(3));
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(2)));
        assert!(w.children[2].is_active());

        // Truncate to two rows — the active row 2 is dropped.
        let truncated: FlexView<Vec<u32>> = column().child(captor(0)).child(captor(1));
        truncated.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);
        assert_eq!(w.flex.len(), 2);
        assert!(w.children.iter().all(|p| !p.is_active()));

        // A release lands nowhere armed → no fire, no panic.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(2)));
        assert!(log.is_empty());
    }

    #[test]
    fn type_swap_at_active_index_clears_without_notifying_fresh_widget() {
        // (Scenario 3) A type swap at the active index clears the stale capture
        // but does NOT deliver anything to the fresh widget — it must see nothing
        // until a new Down.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column().child(captor(0)).child(captor(1));
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // Swap row 1 from Captor to a Recorder (a different concrete type).
        let seen = Rc::new(Cell::new(0u32));
        let swapped: FlexView<Vec<u32>> = column()
            .child(captor(0))
            .child(Recorder { seen: seen.clone() });
        swapped.rebuild(&prev, &mut w, &mut ctx(&mut counter));

        assert!(!w.children[1].is_active(), "stale capture path dropped");
        assert_eq!(seen.get(), 0, "fresh widget received no synthetic event");

        // A brand-new Down now reaches the fresh widget.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(seen.get(), 1, "fresh widget responds to a new gesture");
    }

    #[test]
    fn content_only_rebuild_preserves_captured_drag() {
        // (Scenario 4, the critical negative test) A structural-change-free
        // rebuild (same length, same types) must NOT break a captured drag: the
        // active path survives and the release still fires on the captured row.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column().child(captor(0)).child(captor(1));
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // An ordinary every-frame rebuild: same structure, content only.
        let same: FlexView<Vec<u32>> = column().child(captor(0)).child(captor(1));
        same.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_active(),
            "content-only rebuild must NOT clear an in-flight capture"
        );

        // The captured drag completes and fires on the still-armed row.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert_eq!(log, vec![1], "captured row fires on Up as normal");
    }

    // --- Positional focus-retention fixtures --------------------------------
    //
    // The focus analog of the capture tests above: a `FocusRow` requests focus on
    // `Down` and records its id on a focus-routed `Key` event, so a test can prove
    // both that the pod's `focused` flag survives a sibling structural change (the
    // seed the next paint reads into `PaintCtx::has_focus`) and that the container
    // still routes a `Key` event to the surviving focused row.

    #[test]
    fn focus_on_child_survives_append_after() {
        // Focus on child 0 survives appending a row after it (a count change beyond
        // the focused index): the focused pod stays in the stable prefix, so its
        // `focused` flag — and thus the next paint's `PaintCtx::has_focus` and the
        // published IME surface — stays live, and a Key event still reaches it.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column().child(FocusRow { id: 0 }).child(FocusRow { id: 1 });
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert!(w.children[0].is_focused(), "child 0 took focus");

        // Append a third row AFTER the focused one → length grows, no type swap.
        let appended: FlexView<Vec<u32>> = column()
            .child(FocusRow { id: 0 })
            .child(FocusRow { id: 1 })
            .child(FocusRow { id: 2 });
        appended.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[0].is_focused(),
            "append-after preserves the focused child's recorded path"
        );

        // A Key event still routes to the surviving focused child 0.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &key_event());
        assert_eq!(log, vec![0], "Key still routes to the focused row");
    }

    #[test]
    fn focus_on_child_survives_remove_after() {
        // Symmetric to the append case: removing a row AFTER the focused index
        // (a shrink beyond it) leaves the focused pod in the stable prefix.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column()
            .child(FocusRow { id: 0 })
            .child(FocusRow { id: 1 })
            .child(FocusRow { id: 2 });
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert!(w.children[0].is_focused());

        // Remove the last row → shrink beyond the focused index.
        let removed: FlexView<Vec<u32>> =
            column().child(FocusRow { id: 0 }).child(FocusRow { id: 1 });
        removed.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[0].is_focused(),
            "remove-after preserves the focused child's recorded path"
        );

        layout_column(&mut w);
        dispatch(&mut w, &mut log, &key_event());
        assert_eq!(log, vec![0], "Key still routes to the focused row");
    }

    #[test]
    fn focus_cleared_when_focused_index_type_swaps() {
        // When the focused index itself type-swaps, its widget identity breaks →
        // the focus path is cleared and a subsequent Key event reaches nobody.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = column().child(FocusRow { id: 0 }).child(FocusRow { id: 1 });
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_focused(), "child 1 took focus");

        // Swap the focused index 1 to a different concrete type (a Captor).
        let swapped: FlexView<Vec<u32>> = column().child(FocusRow { id: 0 }).child(captor(9));
        swapped.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            !w.children[1].is_focused(),
            "a type swap at the focused index clears its focus path"
        );

        // No focused pod remains → the Key event is dropped.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &key_event());
        assert!(
            log.is_empty(),
            "Key reaches nobody after the focused index swaps"
        );
    }

    // --- Keyed reconciliation fixtures --------------------------------------
    //
    // A stateful probe row: `CounterWidget` holds an internal `count` that starts
    // at 0 on build and increments on every `Down`, pushing the post-increment
    // value into the `Vec<u32>` app state. Its rebuild deliberately does NOT reset
    // `count`, so the pushed sequence reveals whether a reconciliation *relocated*
    // the live widget (count continues) or *rebuilt* it from scratch (count resets
    // to 1). This is the probe the reorder-preserves-state test turns on.

    /// A stateful counter row view tagged with `id`.
    struct Counter {
        id: u32,
    }
    /// Retained widget for [`Counter`]: `count` survives an in-place rebuild.
    struct CounterWidget {
        id: u32,
        count: u32,
    }

    impl View<Vec<u32>> for Counter {
        type Element = CounterWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterWidget {
            CounterWidget {
                id: self.id,
                count: 0,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CounterWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // Adopt the new id but preserve the accumulated count — a relocated
            // widget must keep its internal state.
            element.id = self.id;
            ChangeFlags::NONE
        }
    }

    impl Widget for CounterWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            if p.phase == PointerPhase::Down {
                self.count += 1;
                ctx.state_mut::<Vec<u32>>().push(self.count);
                EventResult::Handled
            } else {
                EventResult::Ignored
            }
        }
    }

    /// A keyed inflexible counter child.
    fn kcounter(key: u64, id: u32) -> FlexChild<Vec<u32>> {
        keyed(key, Counter { id })
    }

    /// Build a vertical keyed column of counter rows.
    fn keyed_column(children: Vec<FlexChild<Vec<u32>>>) -> FlexView<Vec<u32>> {
        FlexView::new(Axis::Vertical, children)
    }

    #[test]
    fn keyed_reorder_preserves_widget_state() {
        // THE CRITICAL TEST. Two keyed counter rows; drive row A's internal count
        // up, reorder the list, then drive A again — its count must continue from
        // where it left off, proving the reorder relocated A's live widget rather
        // than rebuilding whatever now sits at A's old index.
        let mut counter = 0u64;
        let prev = keyed_column(vec![kcounter(1, 1), kcounter(2, 2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Row A (key 1) at index 0: three Downs → its internal count reaches 3.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert_eq!(log, vec![1, 2, 3], "count accumulates on the original row");
        log.clear();

        // Reorder: [B, A]. Row A moves to index 1.
        let reordered = keyed_column(vec![kcounter(2, 2), kcounter(1, 1)]);
        reordered.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);
        layout_column(&mut w);

        // Drive row A at its NEW index (1). If its widget was relocated, the count
        // continues to 4; a from-scratch rebuild would reset it to 1.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(
            log,
            vec![4],
            "reordered row kept its internal state (4, not a reset 1)"
        );
    }

    #[test]
    fn keyed_insert_above_preserves_existing_widget_state() {
        // Inserting a new keyed row above the existing ones must not rebuild them:
        // the surviving rows relocate (state preserved), only the new key builds.
        let mut counter = 0u64;
        let prev = keyed_column(vec![kcounter(1, 1), kcounter(2, 2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Row with key 2 (index 1): two Downs → count 2.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(log, vec![1, 2]);
        log.clear();

        // Insert a fresh key 9 at the top: [9, 1, 2]. Key 2 shifts to index 2.
        let inserted = keyed_column(vec![kcounter(9, 9), kcounter(1, 1), kcounter(2, 2)]);
        inserted.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 3);
        layout_column(&mut w);

        // Key 2 at its new index (2) continues its count to 3, not a reset 1.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(2)));
        assert_eq!(log, vec![3], "shifted row preserved its state");
        log.clear();

        // The freshly-built key 9 (index 0) starts its own count at 1.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert_eq!(log, vec![1], "newly-inserted key builds a fresh widget");
    }

    #[test]
    fn keyed_swap_preserves_both_widgets() {
        // A straight two-row swap must preserve *both* rows' state.
        let mut counter = 0u64;
        let prev = keyed_column(vec![kcounter(1, 1), kcounter(2, 2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // A (key 1, idx 0) → count 1; B (key 2, idx 1) → count 1 then 2.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(log, vec![1, 1, 2]);
        log.clear();

        // Swap → [B, A].
        let swapped = keyed_column(vec![kcounter(2, 2), kcounter(1, 1)]);
        swapped.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        layout_column(&mut w);

        // B now at idx 0 continues to 3; A now at idx 1 continues to 2.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(log, vec![3, 2], "both swapped rows kept their state");
    }

    #[test]
    fn keyed_same_order_rebuild_is_not_structural() {
        // A same-keys, same-order keyed rebuild is the content-only case: it must
        // NOT clear an in-flight capture (mirrors the positional negative test).
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(2u64, Captor { id: 1 })],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // Same keys, same order → not structural.
        let same: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(2u64, Captor { id: 1 })],
        );
        same.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_active(),
            "same-order keyed rebuild must not clear an in-flight capture"
        );

        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert_eq!(log, vec![1], "captured row fires on Up as normal");
    }

    #[test]
    fn keyed_reorder_preserves_captured_drag() {
        // A key-matched row's identity is intact across a reorder, so its
        // in-flight capture is CARRIED with the relocated pod — not cancelled. The
        // captured drag completes and fires on the row at its new index.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(2u64, Captor { id: 1 })],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Arm the capture in the key-1 row (index 0).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert!(w.children[0].is_active());

        // Reorder → [key2, key1]. The key-1 row relocates to index 1 with its
        // `active` flag intact.
        let reordered: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(2u64, Captor { id: 1 }), keyed(1u64, Captor { id: 0 })],
        );
        reordered.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_active(),
            "keyed reorder carries the captured row's active path to its new index"
        );

        // The captured drag completes and fires on the relocated key-1 row (id 0).
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert_eq!(log, vec![0], "the relocated captured row fires on Up");
    }

    #[test]
    fn keyed_removed_active_key_unwinds_without_fire() {
        // Removing a key whose row holds an in-flight capture tears it down via
        // the cancel-if-active path: no fire, no panic.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(1u64, Captor { id: 0 }),
                keyed(2u64, Captor { id: 1 }),
                keyed(3u64, Captor { id: 2 }),
            ],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Arm the key-2 row (index 1).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // Drop key 2 → [key1, key3].
        let removed: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(3u64, Captor { id: 2 })],
        );
        removed.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);
        assert_eq!(w.flex.len(), 2);
        assert!(w.children.iter().all(|p| !p.is_active()));

        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert!(log.is_empty(), "removed active row does not fire on Up");
    }

    #[test]
    fn keyed_reorder_preserves_focus_and_key_routing() {
        // Focus is the second recorded path and rides along with the
        // relocated pod: a key-matched focused row keeps its focus across a reorder,
        // and the container routes a subsequent Key event to it at its new index.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(1u64, FocusRow { id: 1 }),
                keyed(2u64, FocusRow { id: 2 }),
            ],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Focus the key-1 row (index 0).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert!(w.children[0].is_focused());

        // Reorder → [key2, key1]. The key-1 row relocates to index 1, carrying its
        // focus flag with it.
        let reordered: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(2u64, FocusRow { id: 2 }),
                keyed(1u64, FocusRow { id: 1 }),
            ],
        );
        reordered.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_focused(),
            "the focused key-1 row keeps focus at its new index"
        );
        assert!(!w.children[0].is_focused());

        // A Key event routes to the relocated focused row (id 1).
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &key_event());
        assert_eq!(log, vec![1], "Key routes to the relocated focused row");
    }

    #[test]
    fn keyed_removed_focused_key_clears_focus() {
        // Removing the focused keyed row breaks its identity: the pod is
        // torn down, so no focused pod remains and a subsequent Key reaches nobody.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(1u64, FocusRow { id: 1 }),
                keyed(2u64, FocusRow { id: 2 }),
            ],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Focus the key-2 row (index 1).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_focused());

        // Remove key 2 → only key 1 survives, and it never held focus.
        let removed: FlexView<Vec<u32>> =
            FlexView::new(Axis::Vertical, vec![keyed(1u64, FocusRow { id: 1 })]);
        removed.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 1);
        assert!(
            w.children.iter().all(|p| !p.is_focused()),
            "the removed focused row leaves no focus path behind"
        );

        layout_column(&mut w);
        dispatch(&mut w, &mut log, &key_event());
        assert!(
            log.is_empty(),
            "Key reaches nobody after the focused key is removed"
        );
    }

    /// A focus-taking row: requests focus on `Down`, records `id` on a Key event
    /// (so a test can prove a focus-routed event reaches it at its current index).
    struct FocusRow {
        id: u32,
    }
    /// Retained widget for [`FocusRow`].
    struct FocusRowWidget {
        id: u32,
    }

    impl View<Vec<u32>> for FocusRow {
        type Element = FocusRowWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FocusRowWidget {
            FocusRowWidget { id: self.id }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut FocusRowWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.id = self.id;
            ChangeFlags::NONE
        }
    }

    impl Widget for FocusRowWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    ctx.request_focus();
                    EventResult::Handled
                }
                // A focus-routed Key event records this row's id — how a test
                // observes which row the container routes focus to.
                InputEvent::Key(_) => {
                    ctx.state_mut::<Vec<u32>>().push(self.id);
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    /// Build a focus-routed `Key` event (an "a" keypress).
    fn key_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character("a".to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }
}
