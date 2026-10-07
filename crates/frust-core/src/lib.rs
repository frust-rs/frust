//! Layer 1+2 of Frust: the declarative view API, the retained widget tree,
//! box-constraint layout, and the rebuild/layout/paint pass skeleton.
//!
//! The design mirrors `xilem_core`'s proven `View` lifecycle and Masonry's
//! `tree_arena`-backed widget tree, but owns its implementation — there is **no
//! xilem/masonry dependency and no `unsafe`** in this crate.
//!
//! # Layers
//!
//! * [`view`] — layer 1: the [`View`](view::View) trait, [`ChangeFlags`], and
//!   [`BuildCtx`](view::BuildCtx). Views are cheap descriptors produced by
//!   `fn build(&mut State) -> impl View<State>`. [`ViewSeq`] is
//!   the child-sequence trait container child lists take (a view, a tuple, a
//!   `Vec`/array/`Option`, or [`views`] over an iterator).
//! * [`widget`] — layer 2: the [`Widget`](widget::Widget) trait and its
//!   layout/paint/event contexts, the [`PaintScene`](widget::PaintScene) paint
//!   boundary, and container-owned [`ChildPod`](widget::ChildPod) children.
//! * [`event`] — layer 2 input: [`InputEvent`](event::InputEvent)/
//!   [`PointerEvent`](event::PointerEvent) and the [`EventCtx`](event::EventCtx)
//!   handlers mutate state through.
//! * [`input`] — pure gesture helpers: slop/wheel constants, a
//!   [`VelocityTracker`](input::VelocityTracker), and the fling-decay math the
//!   interactive widgets build on.
//! * [`layout`] — the [`BoxConstraints`](layout::BoxConstraints) box model.
//! * [`tree`] — the [`WidgetTree`](tree::WidgetTree) arena wrapper,
//!   [`WidgetPod`](tree::WidgetPod), and the read-only
//!   [`InspectNode`](tree::InspectNode) walk tooling reads the tree through.
//! * [`app`] — the [`RenderRoot`](app::RenderRoot) that drives rebuild → layout
//!   → paint. This is what each platform shell owns.
//! * [`component`] — [`Component`](component::Component), Flutter's
//!   `StatefulWidget` analog: a subtree with retained local state, a
//!   per-component reactive `Owner`, and a state boundary the outer view tree
//!   never sees.
//! * [`overlay`] — the overlay portal: an owner-hosted, root-painted,
//!   root-routed pod a widget floats above the whole app (menus, popovers,
//!   tooltips, selection toolbars).
//! * [`selection_toolbar`] — what a text field publishes when it has a
//!   selection, plus the process-global policy/builder slots that decide who
//!   draws the toolbar.
//! * [`hit`] — hit-testing helpers for content drawn under an arbitrary
//!   [`kurbo::Affine`]: the test [`ChildPod`](widget::ChildPod) uses for a
//!   transformed pod, exposed for canvas hit closures and pan/zoom containers.

pub mod anim;
pub mod app;
pub mod component;
pub mod event;
#[cfg(feature = "hotpatch")]
pub mod hotpatch;
pub mod input;
pub mod insets;
pub mod layout;
pub mod overlay;
pub mod selection_toolbar;
pub mod semantics;
pub mod tree;
pub mod view;
pub mod widget;

/// Hit-testing under an arbitrary [`kurbo::Affine`].
///
/// [`ChildPod::set_transform`](widget::ChildPod::set_transform) places a child
/// under a transform; these are the helpers it hit-tests with, exposed so a
/// canvas hit closure or a pan/zoom container answers "is this point on that
/// shape" exactly the way the pod does.
pub mod hit {
    use kurbo::{Affine, Point, Rect};

    /// `affine`'s inverse, or `None` when it has none — a singular (zero
    /// determinant) or non-finite transform, or one whose inverse overflows.
    ///
    /// [`Affine::inverse`] on a singular matrix divides by zero and returns
    /// non-finite coefficients rather than failing; this is the checked form
    /// every caller that maps a point back through a transform should use.
    pub fn checked_inverse(affine: &Affine) -> Option<Affine> {
        let det = affine.determinant();
        if det == 0.0 || !det.is_finite() || !affine.is_finite() {
            return None;
        }
        let inverse = affine.inverse();
        inverse.is_finite().then_some(inverse)
    }

    /// Whether `point` lands inside `rect` once `rect` is drawn under `affine`.
    ///
    /// `rect` is in the content's own (local) space and `affine` maps that space
    /// into the space `point` is in, so this is the hit test for content painted
    /// inside `push_transform(affine)`: `point` is mapped back through the
    /// inverse and tested against `rect` with the same half-open convention as
    /// [`ChildPod::contains`](crate::widget::ChildPod::contains) (`x0 <= x < x1`,
    /// `y0 <= y < y1`). A rotated rect therefore hits along its rotated edges,
    /// not its axis-aligned bounding box.
    ///
    /// A transform with no inverse ([`checked_inverse`]) collapses the rect to a
    /// line or a point, which nothing can hit: the answer is `false`, never a
    /// panic.
    pub fn point_in_transformed_rect(point: Point, rect: Rect, affine: &Affine) -> bool {
        let Some(inverse) = checked_inverse(affine) else {
            return false;
        };
        let local = inverse * point;
        local.x >= rect.x0 && local.x < rect.x1 && local.y >= rect.y0 && local.y < rect.y1
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use kurbo::Vec2;
        use std::f64::consts::FRAC_PI_4;

        #[test]
        fn identity_matches_the_half_open_rect_test() {
            let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
            assert!(point_in_transformed_rect(
                Point::ZERO,
                rect,
                &Affine::IDENTITY
            ));
            assert!(point_in_transformed_rect(
                Point::new(9.99, 9.99),
                rect,
                &Affine::IDENTITY
            ));
            assert!(!point_in_transformed_rect(
                Point::new(10.0, 5.0),
                rect,
                &Affine::IDENTITY
            ));
        }

        #[test]
        fn scale_and_translate_map_the_point_back() {
            let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
            let affine = Affine::translate(Vec2::new(100.0, 50.0)) * Affine::scale(2.0);
            // Drawn at (100..120, 50..70).
            assert!(point_in_transformed_rect(
                Point::new(119.0, 69.0),
                rect,
                &affine
            ));
            assert!(!point_in_transformed_rect(
                Point::new(121.0, 60.0),
                rect,
                &affine
            ));
            assert!(!point_in_transformed_rect(
                Point::new(105.0, 49.0),
                rect,
                &affine
            ));
        }

        #[test]
        fn rotation_hits_the_diamond_not_its_bounding_box() {
            let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
            let affine = Affine::rotate_about(FRAC_PI_4, Point::new(50.0, 50.0));
            // The box's own corner (1, 1) is outside the rotated diamond.
            assert!(!point_in_transformed_rect(
                Point::new(1.0, 1.0),
                rect,
                &affine
            ));
            // The diamond's top vertex sits at (50, 50 - 50·√2).
            let top = affine * Point::new(0.0, 0.0);
            assert!(point_in_transformed_rect(
                top + Vec2::new(0.0, 1.0),
                rect,
                &affine
            ));
            assert!(!point_in_transformed_rect(
                top - Vec2::new(0.0, 1.0),
                rect,
                &affine
            ));
        }

        #[test]
        fn a_singular_transform_hits_nothing() {
            let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
            for affine in [
                Affine::scale(0.0),
                Affine::scale_non_uniform(1.0, 0.0),
                Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]),
                Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]),
            ] {
                assert!(checked_inverse(&affine).is_none());
                assert!(!point_in_transformed_rect(Point::ZERO, rect, &affine));
            }
        }
    }
}

/// Re-export of the [`accesskit`] accessibility vocabulary (roles, node
/// builders, toggle/value state) widgets use to populate a
/// [`semantics::SemanticsCtx`]. Re-exported here — the sole crate that depends
/// on accesskit — so `frust-widgets` (and app code) name the vocabulary
/// through `frust_core::accesskit::*` without a direct dependency; the
/// platform `accesskit_*` adapter crates live in the shells, not here.
pub use accesskit;
pub use anim::{
    AnimationController, AnimationStatus, Curve, FrameTime, Lerp, Spring, SpringDesc, Tween,
};
pub use app::{Orientation, RenderRoot, WindowMetrics};
pub use component::{Component, ComponentView, ComponentWidget, component};
pub use event::{
    CursorIcon, EditCommand, EditingState, EventCtx, EventOutcome, EventResult, ImeContentType,
    ImeEvent, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, OverlayEvent,
    OverlayEventKind, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
    has_pending_result_flush, mark_focus_orphaned, mark_pending_result_flush, take_focus_orphaned,
    take_pending_result_flush,
};
#[cfg(feature = "hotpatch")]
pub use hotpatch::set_patch_listener;
pub use input::{
    FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
    WHEEL_LINE_PX, fling_decay, fling_displacement,
};
pub use insets::{CornerInset, CornerInsets, EdgeInsets as WindowEdgeInsets, WindowInsets};
pub use layout::BoxConstraints;
pub use overlay::{
    OutsideTap, OverlayBand, OverlayEntry, OverlayHit, OverlayInput, OverlayKey, OverlayPod,
};
pub use selection_toolbar::{
    SelectionToolbarActions, SelectionToolbarBuilder, SelectionToolbarPolicy,
    SelectionToolbarRequest, install_selection_toolbar_builder_if_unset,
    lock_selection_toolbar_policy, selection_toolbar_builder, selection_toolbar_policy,
    set_selection_toolbar_builder, set_selection_toolbar_policy,
};
pub use semantics::{SemanticsCtx, SemanticsUpdate};
pub use tree::{InspectNode, WidgetPod, WidgetTree};
pub use view::{AnyView, BuildCtx, ChangeFlags, View, ViewSeq, WidgetId, any, views};
pub use widget::{
    ChildPod, CornerRadii, DashPattern, DiscardScene, HeroDirective, HeroFrames, LayoutCtx,
    PaintCtx, PaintOutcome, PaintScene, TickClass, Widget,
};
