//! Layer 1+2 of Frust: the declarative view API, the retained widget tree, box-constraint layout,
//! and the rebuild/layout/paint pass skeleton. Modelled on xilem/Masonry, depending on neither,
//! with no `unsafe`.
//!
//! * [`anim`] — the frame clock, curves, tweens, springs and animation controllers.
//! * [`app`] — the [`RenderRoot`] each platform shell drives through rebuild, layout and paint.
//! * [`mod@component`] — [`Component`], a subtree with retained local state and its own `Owner`.
//! * [`event`] — input events and the [`EventCtx`] handlers mutate state through.
//! * `hotpatch` (feature `hotpatch`) — the hot-patch seam every `Component::build` runs through.
//! * [`hit`] — hit tests for content drawn under an arbitrary [`kurbo::Affine`].
//! * [`input`] — slop/wheel constants, [`VelocityTracker`] and fling-decay math.
//! * [`insets`] — the [`WindowInsets`] safe-area and keyboard inset model.
//! * [`layout`] — the [`BoxConstraints`] box model.
//! * [`overlay`] — the portal a widget floats a pod above the whole app through.
//! * [`retained`] — retained-page keys and the renderer's resident-key ledger.
//! * [`selection_toolbar`] — a text field's toolbar request and the policy/builder slots.
//! * [`semantics`] — the accessibility tree pass behind [`SemanticsUpdate`].
//! * [`shortcuts`] — the [`Chord`] spec and the per-paint keyboard-shortcut declarations.
//! * [`sliver`] — the sliver protocol a scrolling viewport lays children out with.
//! * [`tree`] — the [`WidgetTree`] arena and the read-only [`InspectNode`] walk.
//! * [`view`] — the [`View`] trait, [`ChangeFlags`], [`BuildCtx`] and [`ViewSeq`].
//! * [`widget`] — the [`Widget`] trait, its contexts, [`PaintScene`] and [`ChildPod`].

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
pub mod retained;
pub mod selection_toolbar;
pub mod semantics;
pub mod shortcuts;
pub mod sliver;
pub mod tree;
pub mod view;
pub mod widget;

/// Hit-testing under an arbitrary [`kurbo::Affine`]: the helpers a transformed
/// [`ChildPod`] uses, for canvas hit closures and pan/zoom containers.
pub mod hit {
    use kurbo::{Affine, Point, Rect};

    /// `affine`'s inverse, or `None` for a singular or non-finite transform or an
    /// overflowing inverse. Use it instead of [`Affine::inverse`], which returns
    /// non-finite coefficients for a singular matrix rather than failing.
    pub fn checked_inverse(affine: &Affine) -> Option<Affine> {
        let det = affine.determinant();
        if det == 0.0 || !det.is_finite() || !affine.is_finite() {
            return None;
        }
        let inverse = affine.inverse();
        inverse.is_finite().then_some(inverse)
    }

    /// Whether `point` hits local-space `rect` drawn under `affine` (content inside
    /// `push_transform(affine)`): `point` is mapped back and tested half-open, like
    /// `ChildPod::contains`, so a rotated rect hits along its rotated edges. A
    /// non-invertible `affine` hits nothing.
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

/// The [`accesskit`] vocabulary widgets fill a [`semantics::SemanticsCtx`] with,
/// re-exported so `frust-widgets` and apps use it without their own dependency;
/// the platform `accesskit_*` adapters live in the shells.
pub use accesskit;
pub use anim::{
    AnimationController, AnimationStatus, Curve, FrameTime, Lerp, Spring, SpringDesc, Tween,
};
pub use app::{FocusRingPolicy, Orientation, RenderRoot, WindowMetrics};
pub use component::{Component, ComponentView, ComponentWidget, component};
pub use event::{
    CursorIcon, EditCommand, EditingState, EventCtx, EventOutcome, EventResult, ImeContentType,
    ImeEvent, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, OverlayEvent,
    OverlayEventKind, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
    has_pending_result_flush, mark_focus_orphaned, mark_pending_result_flush, take_focus_orphaned,
    take_pending_result_flush,
};
#[cfg(feature = "hotpatch")]
pub use hotpatch::{LayoutMismatch, SeamWitness, build_erased, set_patch_listener};
pub use input::{
    FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
    WHEEL_LINE_PX, fling_decay, fling_displacement,
};
pub use insets::{CornerInset, CornerInsets, EdgeInsets as WindowEdgeInsets, WindowInsets};
pub use layout::BoxConstraints;
pub use overlay::{
    OutsideTap, OverlayBand, OverlayEntry, OverlayHit, OverlayInput, OverlayKey, OverlayPod,
};
pub use retained::{RetainedEntry, RetainedKey, RetainedLedger};
pub use selection_toolbar::{
    SelectionToolbarActions, SelectionToolbarBuilder, SelectionToolbarPolicy,
    SelectionToolbarRequest, install_selection_toolbar_builder_if_unset,
    lock_selection_toolbar_policy, selection_toolbar_builder, selection_toolbar_policy,
    set_selection_toolbar_builder, set_selection_toolbar_policy,
};
pub use semantics::{SemanticsCtx, SemanticsUpdate};
pub use shortcuts::{Chord, ChordParseError, ShortcutDecl, ShortcutId, ShortcutScope};
pub use sliver::{
    GrowthDirection, ScrollAxis, ScrollDirection, SliverConstraints, SliverGeometry, SliverWidget,
};
pub use tree::{InspectNode, WidgetPod, WidgetTree};
pub use view::{AnyView, BuildCtx, ChangeFlags, View, ViewSeq, WidgetId, any, views};
pub use widget::{
    ChildPod, CornerRadii, DashPattern, DiscardScene, HeroDirective, HeroFrames, LayoutCtx,
    PaintCtx, PaintOutcome, PaintScene, TickClass, Widget,
};
