//! Integration tests for [`frust_widgets::scaffold`]'s layout contract,
//! driven through a real [`RenderRoot`] rebuild + layout so window insets can
//! be simulated the same way [`RenderRoot::set_insets`]'s other consumers are
//! tested (`safe_area.rs`'s `#[cfg(test)]` module is the sibling precedent).
//!
//! Fixtures are hand-written (no `test-support` feature dependency) mirroring
//! this crate's other integration tests (`visit_children.rs`,
//! `semantics_tree.rs`): a fixed-size leaf per slot role (kept as distinct
//! types so [`find`] can pick one out unambiguously by its type name) plus
//! [`SelfInsetBar`], a bar that reads a window inset edge in its own
//! `layout` and grows by it — the minimal stand-in for
//! `frust_glyph::appbar::AppBarWidget`'s self-sizing contract, which
//! R-B4-inset (`scaffold.rs`'s module docs) is written against.
//!
//! Every fixture's `rebuild` is diff-aware (compares `prev` against `self`,
//! mirroring `frust_widgets::test_support::Leaf`'s own contract) rather than
//! unconditionally returning `ChangeFlags::LAYOUT` — the single-build tests in
//! this file never invoke `rebuild` at all (their one pass is always the
//! unconditional first-build arm), but the second-rebuild coverage further
//! down does, and an unconditionally-dirty fixture would mask whatever
//! `ScaffoldWidget`'s own slot reconciliation (`scaffold.rs`'s private
//! `rebuild_optional_slot`) actually reports.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, InspectNode, LayoutCtx, PaintCtx, PaintScene,
    RenderRoot, View, Widget, WindowEdgeInsets, WindowInsets, any,
};
use frust_widgets::{Alignment, ScaffoldView, scaffold};
use kurbo::{Point, Size};

// -- Fixtures --------------------------------------------------------------

/// The scaffold's required `body` slot: a fixed-size leaf.
struct Body {
    intrinsic: Size,
}
fn body(width: f64, height: f64) -> Body {
    Body {
        intrinsic: Size::new(width, height),
    }
}
struct BodyWidget {
    intrinsic: Size,
}
impl View<()> for Body {
    type Element = BodyWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BodyWidget {
        BodyWidget {
            intrinsic: self.intrinsic,
        }
    }
    fn rebuild(&self, prev: &Self, el: &mut BodyWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        if prev.intrinsic != self.intrinsic {
            el.intrinsic = self.intrinsic;
            ChangeFlags::LAYOUT
        } else {
            ChangeFlags::NONE
        }
    }
}
impl Widget for BodyWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.intrinsic)
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

/// A plain (non-self-insetting) bar, for tests that don't exercise
/// R-B4-inset — a distinct type from [`Body`]/[`Fab`] so [`find`] can locate
/// it unambiguously.
struct Bar {
    height: f64,
}
fn bar(height: f64) -> Bar {
    Bar { height }
}
struct BarWidget {
    height: f64,
}
impl View<()> for Bar {
    type Element = BarWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BarWidget {
        BarWidget {
            height: self.height,
        }
    }
    fn rebuild(&self, prev: &Self, el: &mut BarWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        if prev.height != self.height {
            el.height = self.height;
            ChangeFlags::LAYOUT
        } else {
            ChangeFlags::NONE
        }
    }
}
impl Widget for BarWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        bc.constrain(Size::new(width, self.height))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

/// The `fab` slot: a fixed-size leaf, a distinct type from [`Body`]/[`Bar`].
struct Fab {
    intrinsic: Size,
}
fn fab_leaf(width: f64, height: f64) -> Fab {
    Fab {
        intrinsic: Size::new(width, height),
    }
}
struct FabWidget {
    intrinsic: Size,
}
impl View<()> for Fab {
    type Element = FabWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FabWidget {
        FabWidget {
            intrinsic: self.intrinsic,
        }
    }
    fn rebuild(&self, prev: &Self, el: &mut FabWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        if prev.intrinsic != self.intrinsic {
            el.intrinsic = self.intrinsic;
            ChangeFlags::LAYOUT
        } else {
            ChangeFlags::NONE
        }
    }
}
impl Widget for FabWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.intrinsic)
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

/// Which window edge [`SelfInsetBar`] reads.
#[derive(Clone, Copy, PartialEq)]
enum InsetEdge {
    Top,
    Bottom,
}

/// A bar that self-sizes exactly like
/// `frust_glyph::appbar::AppBarWidget::layout` does: a fixed `content_height`
/// plus whichever inset edge it reads from `ctx.window_insets()` — the
/// minimal fixture R-B4-inset's contract needs, without a `frust-glyph`
/// dependency (`frust-widgets` has none).
struct SelfInsetBar {
    content_height: f64,
    edge: InsetEdge,
}
fn self_inset_bar(content_height: f64, edge: InsetEdge) -> SelfInsetBar {
    SelfInsetBar {
        content_height,
        edge,
    }
}
struct SelfInsetBarWidget {
    content_height: f64,
    edge: InsetEdge,
}
impl View<()> for SelfInsetBar {
    type Element = SelfInsetBarWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SelfInsetBarWidget {
        SelfInsetBarWidget {
            content_height: self.content_height,
            edge: self.edge,
        }
    }
    fn rebuild(
        &self,
        prev: &Self,
        el: &mut SelfInsetBarWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.content_height != self.content_height || prev.edge != self.edge {
            el.content_height = self.content_height;
            el.edge = self.edge;
            ChangeFlags::LAYOUT
        } else {
            ChangeFlags::NONE
        }
    }
}
impl Widget for SelfInsetBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inset = match self.edge {
            InsetEdge::Top => ctx.window_insets().padding().top,
            InsetEdge::Bottom => ctx.window_insets().padding().bottom,
        };
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        bc.constrain(Size::new(width, self.content_height + inset))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

// -- Harness -----------------------------------------------------------------

/// Rebuild, push `insets`, lay out against `window`, and return the inspect
/// snapshot — mirrors `safe_area.rs`'s own test ordering (`set_insets` after
/// `rebuild`, before `layout`).
fn inspect<V: View<()>>(
    build: &mut impl FnMut(&mut ()) -> V,
    window: Size,
    insets: WindowInsets,
) -> Vec<InspectNode> {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    let mut state = ();
    root.rebuild(build, &mut state);
    root.set_insets(insets);
    root.layout(window);
    root.inspect()
}

/// Like [`inspect`], but rebuilds *twice* against `first` then `second` — each
/// a fresh view the way a build closure re-runs every frame — running a
/// REAL second [`RenderRoot::layout`] pass in between, rather than the single
/// build-then-layout pass every test above exercises. Returns both inspect
/// snapshots plus the [`ChangeFlags`] the **second** rebuild reported (the
/// first pass's flags are always the unconditional `LAYOUT | PAINT` of a first
/// build, so only the second pass's flags say anything about `ScaffoldWidget`'s
/// own slot reconciliation — the surface this file's second-rebuild tests
/// exist to cover, `scaffold.rs`'s `rebuild_optional_slot`).
fn two_pass<V: View<()>>(
    mut first: impl FnMut(&mut ()) -> V,
    mut second: impl FnMut(&mut ()) -> V,
    window: Size,
    insets: WindowInsets,
) -> (Vec<InspectNode>, Vec<InspectNode>, ChangeFlags) {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    let mut state = ();
    root.rebuild(&mut first, &mut state);
    root.set_insets(insets);
    root.layout(window);
    let before = root.inspect();

    let flags = root.rebuild(&mut second, &mut state);
    root.layout(window);
    let after = root.inspect();
    (before, after, flags)
}

/// Each node's bare type name (no module path/generics) — `visit_children.rs`'s
/// helper of the same shape.
fn short_names(nodes: &[InspectNode]) -> Vec<&str> {
    nodes
        .iter()
        .map(|n| {
            let bare = n.type_name.split('<').next().unwrap_or(n.type_name);
            bare.rsplit("::").next().unwrap_or(bare)
        })
        .collect()
}

/// The one node whose bare type name is `name` — panics (with the observed
/// names) if there isn't exactly the fixture the caller expects to find.
fn find<'a>(nodes: &'a [InspectNode], name: &str) -> &'a InspectNode {
    nodes
        .iter()
        .find(|n| {
            let bare = n.type_name.split('<').next().unwrap_or(n.type_name);
            bare.rsplit("::").next().unwrap_or(bare) == name
        })
        .unwrap_or_else(|| panic!("no {name} node found; nodes were {:?}", short_names(nodes)))
}

// -- Empty-slot case ---------------------------------------------------------

#[test]
fn body_only_scaffold_has_no_optional_slot_nodes() {
    let nodes = inspect(
        &mut |_: &mut ()| -> ScaffoldView<()> { scaffold(body(50.0, 50.0)) },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    assert_eq!(short_names(&nodes), vec!["ScaffoldWidget", "BodyWidget"]);
    assert_eq!(nodes[0].children.len(), 1);
    let body_node = find(&nodes, "BodyWidget");
    assert_eq!(body_node.bounds.origin(), Point::ZERO);
    assert_eq!(body_node.bounds.width(), 300.0);
    assert_eq!(body_node.bounds.height(), 400.0);
}

// -- Slot sizing/order, all four together ------------------------------------

#[test]
fn all_four_slots_are_present_sized_and_positioned_consistently() {
    let nodes = inspect(
        &mut |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0))
                .app_bar(any(bar(56.0)))
                .bottom_bar(any(bar(48.0)))
                .fab(any(fab_leaf(56.0, 56.0)))
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    // visit_children! declares (body, app_bar, bottom_bar, fab) — pre-order
    // in `inspect()` mirrors paint order, so this also pins the z-stack.
    assert_eq!(
        short_names(&nodes),
        vec![
            "ScaffoldWidget",
            "BodyWidget",
            "BarWidget",
            "BarWidget",
            "FabWidget"
        ]
    );

    let body_node = &nodes[1];
    let app_bar_node = &nodes[2];
    let bottom_bar_node = &nodes[3];
    let fab_node = &nodes[4];

    assert_eq!(app_bar_node.bounds.origin(), Point::ZERO);
    assert_eq!(app_bar_node.bounds.height(), 56.0);

    assert_eq!(body_node.bounds.origin(), Point::new(0.0, 56.0));
    assert_eq!(body_node.bounds.width(), 300.0);
    assert_eq!(body_node.bounds.height(), 400.0 - 56.0 - 48.0);

    assert_eq!(
        bottom_bar_node.bounds.origin(),
        Point::new(0.0, 400.0 - 48.0)
    );
    assert_eq!(bottom_bar_node.bounds.height(), 48.0);

    // Default bottom-right alignment, `fab_margin` inset from each edge; the
    // fab's box already stops above the bottom bar, so no window inset is
    // added on top of it (zero here regardless — no inset was pushed).
    assert_eq!(
        fab_node.bounds.origin(),
        Point::new(300.0 - 16.0 - 56.0, 400.0 - 48.0 - 16.0 - 56.0)
    );
}

#[test]
fn fab_alignment_moves_the_fab_within_its_box() {
    let nodes = inspect(
        &mut |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0))
                .fab(any(fab_leaf(56.0, 56.0)))
                .fab_alignment(Alignment::TOP_LEFT)
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    let fab_node = find(&nodes, "FabWidget");
    assert_eq!(fab_node.bounds.origin(), Point::new(16.0, 16.0));
}

// -- R-B4-inset: the Scaffold never pre-insets a self-insetting app bar ------

#[test]
fn r_b4_inset_a_self_insetting_app_bar_is_never_pre_inset_by_the_scaffold() {
    // A 44px status-bar-style top inset, no IME.
    let insets = WindowInsets::new(
        WindowEdgeInsets::new(0.0, 44.0, 0.0, 0.0),
        WindowEdgeInsets::ZERO,
    );
    let nodes = inspect(
        &mut |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).app_bar(any(self_inset_bar(56.0, InsetEdge::Top)))
        },
        Size::new(300.0, 400.0),
        insets,
    );
    let app_bar_node = find(&nodes, "SelfInsetBarWidget");
    let body_node = find(&nodes, "BodyWidget");

    // The bar already grew by the 44px inset in its own layout.
    assert_eq!(app_bar_node.bounds.height(), 56.0 + 44.0);
    assert_eq!(app_bar_node.bounds.origin(), Point::ZERO);

    // The body sits exactly at the bar's own (already-inset) height — never
    // `44 + bar_height`, which would double-consume the inset.
    assert_eq!(body_node.bounds.origin().y, 100.0);
    assert_eq!(body_node.bounds.origin().y, app_bar_node.bounds.height());
}

// -- FAB bottom-inset consumption --------------------------------------------

#[test]
fn fab_consumes_the_bottom_window_inset_when_no_bottom_bar_is_present() {
    // A 34px home-indicator-style bottom inset, no IME.
    let insets = WindowInsets::new(
        WindowEdgeInsets::new(0.0, 0.0, 0.0, 34.0),
        WindowEdgeInsets::ZERO,
    );
    let nodes = inspect(
        &mut |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).fab(any(fab_leaf(56.0, 56.0)))
        },
        Size::new(300.0, 400.0),
        insets,
    );
    let fab_node = find(&nodes, "FabWidget");
    // `fab_margin` (16) + the bottom inset (34), off the raw bottom edge.
    assert_eq!(
        fab_node.bounds.origin().y,
        400.0 - 16.0 - 34.0 - 56.0,
        "with no bottom_bar, the fab must consume the window's bottom inset itself"
    );
}

// -- bottom_bar: self-insets, and the fab floats above it with no double inset --

#[test]
fn bottom_bar_self_insets_and_the_fab_floats_above_it_without_double_consuming() {
    // The same 34px bottom inset as above, but now claimed by a self-insetting
    // bottom_bar instead of the fab.
    let insets = WindowInsets::new(
        WindowEdgeInsets::new(0.0, 0.0, 0.0, 34.0),
        WindowEdgeInsets::ZERO,
    );
    let nodes = inspect(
        &mut |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0))
                .bottom_bar(any(self_inset_bar(48.0, InsetEdge::Bottom)))
                .fab(any(fab_leaf(56.0, 56.0)))
        },
        Size::new(300.0, 400.0),
        insets,
    );
    let bottom_bar_node = find(&nodes, "SelfInsetBarWidget");
    let body_node = find(&nodes, "BodyWidget");
    let fab_node = find(&nodes, "FabWidget");

    // The bottom bar grew by the 34px inset in its own layout (mirroring the
    // app-bar convention).
    let bar_height = 48.0 + 34.0;
    assert_eq!(bottom_bar_node.bounds.height(), bar_height);
    assert_eq!(
        bottom_bar_node.bounds.origin(),
        Point::new(0.0, 400.0 - bar_height)
    );

    // The body stops exactly above the (already-inset) bottom bar.
    assert_eq!(body_node.bounds.height(), 400.0 - bar_height);

    // The fab floats `fab_margin` above the bottom bar — no *additional*
    // window inset on top of it (that would double-consume the same 34px the
    // bottom bar already self-sized for).
    assert_eq!(
        fab_node.bounds.origin().y,
        400.0 - bar_height - 16.0 - 56.0,
        "the fab must not double-consume the bottom inset once a bottom_bar has self-inset for it"
    );
}

// -- Second-rebuild slot reconciliation ---------------------------------------
//
// Every test above builds and lays out exactly once. The three optional slots
// (`app_bar`/`bottom_bar`/`fab`) are reconciled by `scaffold.rs`'s private
// `rebuild_optional_slot` — the route-derived-chrome pattern `app_bar_opt`/
// `bottom_bar_opt` exist for — which a single build+layout pass never
// exercises at all (the first rebuild always takes the unconditional
// first-build arm). These run a REAL second rebuild, with a REAL second
// layout pass, through [`two_pass`].

#[test]
fn app_bar_removed_on_rebuild_expands_the_body_to_full_height() {
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).app_bar_opt(Some(any(bar(56.0))))
        },
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).app_bar_opt(None::<AnyView<()>>)
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    let body_before = find(&before, "BodyWidget");
    assert_eq!(body_before.bounds.origin(), Point::new(0.0, 56.0));
    assert_eq!(body_before.bounds.height(), 400.0 - 56.0);

    assert_eq!(
        short_names(&after),
        vec!["ScaffoldWidget", "BodyWidget"],
        "the app_bar node must be torn down, not merely emptied"
    );
    let body_after = find(&after, "BodyWidget");
    assert_eq!(body_after.bounds.origin(), Point::ZERO);
    assert_eq!(body_after.bounds.height(), 400.0);
    assert!(flags.needs_layout());
}

#[test]
fn app_bar_added_on_rebuild_shrinks_the_body_below_it() {
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).app_bar_opt(None::<AnyView<()>>)
        },
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).app_bar_opt(Some(any(bar(56.0))))
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    assert_eq!(short_names(&before), vec!["ScaffoldWidget", "BodyWidget"]);
    let body_before = find(&before, "BodyWidget");
    assert_eq!(body_before.bounds.origin(), Point::ZERO);
    assert_eq!(body_before.bounds.height(), 400.0);

    let bar_after = find(&after, "BarWidget");
    assert_eq!(bar_after.bounds.origin(), Point::ZERO);
    assert_eq!(bar_after.bounds.height(), 56.0);
    let body_after = find(&after, "BodyWidget");
    assert_eq!(body_after.bounds.origin(), Point::new(0.0, 56.0));
    assert_eq!(body_after.bounds.height(), 400.0 - 56.0);
    assert!(flags.needs_layout());
}

#[test]
fn bottom_bar_removed_on_rebuild_expands_the_body_to_full_height() {
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).bottom_bar_opt(Some(any(bar(48.0))))
        },
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).bottom_bar_opt(None::<AnyView<()>>)
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    let body_before = find(&before, "BodyWidget");
    assert_eq!(body_before.bounds.origin(), Point::ZERO);
    assert_eq!(body_before.bounds.height(), 400.0 - 48.0);

    assert_eq!(
        short_names(&after),
        vec!["ScaffoldWidget", "BodyWidget"],
        "the bottom_bar node must be torn down, not merely emptied"
    );
    let body_after = find(&after, "BodyWidget");
    assert_eq!(body_after.bounds.origin(), Point::ZERO);
    assert_eq!(body_after.bounds.height(), 400.0);
    assert!(flags.needs_layout());
}

#[test]
fn bottom_bar_added_on_rebuild_shrinks_the_body_above_it() {
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).bottom_bar_opt(None::<AnyView<()>>)
        },
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).bottom_bar_opt(Some(any(bar(48.0))))
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    let body_before = find(&before, "BodyWidget");
    assert_eq!(body_before.bounds.height(), 400.0);

    let bar_after = find(&after, "BarWidget");
    assert_eq!(bar_after.bounds.height(), 48.0);
    assert_eq!(bar_after.bounds.origin(), Point::new(0.0, 400.0 - 48.0));
    let body_after = find(&after, "BodyWidget");
    assert_eq!(body_after.bounds.origin(), Point::ZERO);
    assert_eq!(body_after.bounds.height(), 400.0 - 48.0);
    assert!(flags.needs_layout());
}

#[test]
fn fab_appearing_on_rebuild_positions_the_fab_without_disturbing_the_body() {
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> { scaffold(body(0.0, 0.0)) },
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).fab(any(fab_leaf(56.0, 56.0)))
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    assert_eq!(short_names(&before), vec!["ScaffoldWidget", "BodyWidget"]);
    let body_before = find(&before, "BodyWidget");

    assert_eq!(
        short_names(&after),
        vec!["ScaffoldWidget", "BodyWidget", "FabWidget"]
    );
    let body_after = find(&after, "BodyWidget");
    let fab_after = find(&after, "FabWidget");
    assert_eq!(
        body_before.bounds, body_after.bounds,
        "the body must not move or resize when the fab appears"
    );
    assert_eq!(
        fab_after.bounds.origin(),
        Point::new(300.0 - 16.0 - 56.0, 400.0 - 16.0 - 56.0)
    );
    assert!(flags.needs_layout());
}

#[test]
fn fab_disappearing_on_rebuild_leaves_the_body_undisturbed() {
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(0.0, 0.0)).fab(any(fab_leaf(56.0, 56.0)))
        },
        |_: &mut ()| -> ScaffoldView<()> { scaffold(body(0.0, 0.0)) },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    assert_eq!(
        short_names(&before),
        vec!["ScaffoldWidget", "BodyWidget", "FabWidget"]
    );
    let body_before = find(&before, "BodyWidget");

    assert_eq!(
        short_names(&after),
        vec!["ScaffoldWidget", "BodyWidget"],
        "the fab node must be torn down, not merely emptied"
    );
    let body_after = find(&after, "BodyWidget");
    assert_eq!(
        body_before.bounds, body_after.bounds,
        "the body must not move or resize when the fab disappears"
    );
    assert!(flags.needs_layout());
}

#[test]
fn app_bar_content_swap_with_a_different_height_relayouts_the_body() {
    // Same slot, same view *type* (`Bar`), different `height` — the reconciler
    // takes the in-place-rebuild arm (`AnyView`'s type stays the same), so this
    // exercises `rebuild_optional_slot`'s `(Some, Some, Some(pod))` arm, not a
    // teardown/build swap.
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> { scaffold(body(0.0, 0.0)).app_bar(any(bar(56.0))) },
        |_: &mut ()| -> ScaffoldView<()> { scaffold(body(0.0, 0.0)).app_bar(any(bar(80.0))) },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    let bar_before = find(&before, "BarWidget");
    assert_eq!(bar_before.bounds.height(), 56.0);
    let body_before = find(&before, "BodyWidget");
    assert_eq!(body_before.bounds.origin(), Point::new(0.0, 56.0));

    let bar_after = find(&after, "BarWidget");
    assert_eq!(bar_after.bounds.height(), 80.0);
    let body_after = find(&after, "BodyWidget");
    assert_eq!(body_after.bounds.origin(), Point::new(0.0, 80.0));
    assert_eq!(body_after.bounds.height(), 400.0 - 80.0);
    assert!(flags.needs_layout());
}

#[test]
fn rebuild_with_unchanged_slots_reports_no_change_flags() {
    // All three optional slots present, and every field identical between the
    // two passes — the skip path REVIEW_FOCUS calls out for this hot-spot
    // class: a rebuild that changes nothing must not report LAYOUT/PAINT, or a
    // shell driven off `RenderRoot::has_pending_change_flags`/
    // `take_change_flags` would relayout every frame for nothing.
    let (before, after, flags) = two_pass(
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(50.0, 50.0))
                .app_bar_opt(Some(any(bar(56.0))))
                .bottom_bar_opt(Some(any(bar(48.0))))
                .fab(any(fab_leaf(56.0, 56.0)))
        },
        |_: &mut ()| -> ScaffoldView<()> {
            scaffold(body(50.0, 50.0))
                .app_bar_opt(Some(any(bar(56.0))))
                .bottom_bar_opt(Some(any(bar(48.0))))
                .fab(any(fab_leaf(56.0, 56.0)))
        },
        Size::new(300.0, 400.0),
        WindowInsets::default(),
    );
    assert_eq!(
        before, after,
        "no node should move, resize, appear, or disappear when every slot is unchanged"
    );
    assert!(
        flags.is_empty(),
        "a rebuild with unchanged slots must report no ChangeFlags at all, got {flags:?}"
    );
}
