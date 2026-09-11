//! Baseline widget set: Text, Button, Image, Column/Row, Stack, ScrollView, etc.
//!
//! Ships the [`text`] leaf plus the primitive layout containers —
//! [`Row`]/[`Column`] ([`FlexView`]), [`Stack`], [`Padding`], [`Align`], and
//! [`SizedBox`] — built as `View`/`Widget` pairs over `frust-core`'s
//! [`AnyView`](frust_core::AnyView)/[`ChildPod`](frust_core::ChildPod)
//! substrate. Containers own their children directly as `ChildPod`s (the arena
//! stays single-root); see [`frust_core::widget::ChildPod`] for the rationale.
//!
//! # Shared child plumbing
//!
//! The container modules build/rebuild/teardown their heterogeneous children
//! through the [`authoring::build_child`]/[`authoring::rebuild_child`]/
//! [`authoring::teardown_child`] helpers and route pointer events through
//! [`authoring::route_event`] (multi-child containers — `Flex`/`Stack`) or
//! [`authoring::route_event_single`] (one-child wrappers —
//! `Padding`/`Align`/`SizedBox`). Each child is an
//! [`AnyView`](frust_core::AnyView) whose element (`Box<dyn Widget>`) is stored
//! double-boxed inside a `ChildPod`, so a later rebuild can recover
//! `&mut Box<dyn Widget>` to drive `AnyView`'s type-erased reconciliation.
//!
//! That toolkit is **public** — see the [`authoring`] module — so a design
//! system is authored outside this crate against exactly the surface the
//! baseline widgets use themselves. No design-system catalog lives here any
//! more: the three built-in ones ship as their own plugin crates
//! (`frust-glyph`/`frust-material`/`frust-cupertino`), each depending on the
//! `frust` facade alone, so the authoring boundary is now enforced by the
//! crate graph rather than by a source scan.

mod align;
pub mod authoring;
mod button;
mod checkbox;
mod container;
mod divider;
mod flex;
mod gesture;
mod icon;
mod icon_button;
pub mod icons;
mod image;
mod list_view;
pub mod motion;
pub mod nav;
mod overlay;
mod padding;
pub mod physics;
mod platform_view;
mod radio;
mod safe_area;
mod scaffold;
mod scroll;
mod sized;
mod slider;
mod stack;
mod text;
mod textinput;

use std::hash::{Hash, Hasher};

pub use align::{Align, AlignView, AlignWidget, Alignment};
pub use button::{Button, ButtonStyle, ButtonView, ButtonWidget, button};
pub use checkbox::{Checkbox, CheckboxView, CheckboxWidget, checkbox};
pub use container::{BorderStyle, ContainerView, ContainerWidget, colored_box, container};
pub use divider::{DividerView, DividerWidget, divider};
pub use flex::{
    Axis, Column, CrossAxisAlignment, FlexChild, FlexView, FlexWidget, MainAxisAlignment, Row,
    flexible, inflexible, keyed,
};
pub use gesture::{GestureDetector, GestureDetectorView, GestureDetectorWidget};
pub use icon::{Icon, IconData, IconSource, IconView, IconWidget, icon};
pub use icon_button::{IconButton, IconButtonView, IconButtonWidget, icon_button};
pub use image::{Image, ImageError, ImageFit, ImageSource, ImageView, ImageWidget};
pub use list_view::{ListView, ListViewWidget, list_view};
pub use nav::hero::{HeroView, HeroWidget, hero};
pub use nav::navigator::{
    BackPolicy, NavigatorController, NavigatorId, NavigatorView, NavigatorWidget, PageBuilder,
    PageVisibility, PopResult, PushOptions, ReplaceOptions, ResultCallback, RouteChangeCallback,
    VisibilityCallback, navigator, overlay_host,
};
pub use nav::path::{Location, PathPattern, RouteParams};
pub use nav::route::{NavRequest, NavWaker, RouteNavigator};
pub use nav::route_state::{NavChange, RouteStack};
pub use nav::router::{
    DEFAULT_REDIRECT_LIMIT, ErrorBuilder, Redirect, Resolution, ResolvedPage, Route, RouteBuilder,
    Router, shell_route,
};
pub use nav::transition::{PageTransition, Timing, TransitionSpec, TransitionState};
pub use overlay::{
    DEFAULT_OFFSET, DEFAULT_PADDING, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlayPortalView, OverlayPortalWidget, OverlaySide, OverlaySlot, overlay_portal, place,
};
pub use padding::{EdgeInsets, Padding, PaddingView, PaddingWidget};
pub use physics::effect::OverscrollEffect;
pub use physics::parity::{
    AlwaysScrollable, Bouncing, Clamping, DecelerationRate, NeverScrollable,
};
pub use physics::rubber_band::RubberBand;
pub use physics::{
    MAX_FLING_VELOCITY, MIN_FLING_VELOCITY, ScrollMetrics, ScrollPhysics, Simulation,
    SpringDescription, Tolerance,
};
pub use platform_view::{
    PlatformViewView, PlatformViewWidget, ShieldView, ShieldWidget, platform_view, shield,
};
pub use radio::{Radio, RadioView, RadioWidget, radio};
pub use safe_area::{SafeAreaView, SafeAreaWidget, safe_area};
pub use scaffold::{ScaffoldView, ScaffoldWidget, scaffold};
pub use scroll::{ScrollInfo, ScrollView, ScrollWidget, scroll_view};
pub use sized::{SizedBox, SizedBoxView, SizedBoxWidget};
pub use slider::{Slider, SliderView, SliderWidget, slider};
pub use stack::{Stack, StackView, StackWidget};
pub use text::{TextView, TextWidget, text};
pub use textinput::{TextInput, TextInputView, TextInputWidget, text_input};

/// A stable identity for a list child, so a container's reconciliation can match
/// a child to its live widget *by key* across reorders/inserts instead of by
/// position — the difference between "the third row's widget" and "row #42's
/// widget" when the list is shuffled.
///
/// Built from any [`Hash`] value (an item id, a string name, an index) via the
/// `From` impls below and [`keyed`](crate::keyed); the hashed `u64` is what the
/// reconciler compares. Two children in the same list must not collide — a
/// duplicate key is a `debug_assert` tripwire that falls back to positional
/// reconciliation (see [`authoring::rebuild_children`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChildKey(u64);

impl ChildKey {
    /// Hash any [`Hash`] value into a `ChildKey`. Backs the `From` impls and
    /// [`keyed`](crate::keyed)'s `impl Into<ChildKey>` argument.
    pub fn new(value: impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        ChildKey(hasher.finish())
    }
}

macro_rules! child_key_from {
    ($($t:ty),* $(,)?) => {
        $(
            impl From<$t> for ChildKey {
                fn from(value: $t) -> Self {
                    ChildKey::new(value)
                }
            }
        )*
    };
}

// Common key types: integer ids/indices, chars, and string names. A blanket
// `impl<T: Hash> From<T>` would collide with the reflexive `From<ChildKey>`, so
// the ergonomic conversions are spelled out for the types keys are drawn from.
child_key_from!(
    u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, char, &str, String
);

/// Shared, GPU-free fixtures for the container layout/paint/event tests: a
/// fixed-size [`Leaf`], a distinctive swap partner, an event-recording
/// [`Probe`], and a recording [`RecordingScene`].
///
/// Compiled into this crate's own test build, and — behind the non-default
/// `test-support` feature — into the library itself, so a design system authored
/// against [`authoring`] outside this crate can test its containers against the
/// same fixtures this crate's own container tests use — the design-system
/// plugin crates are exactly that consumer. The feature is off by default: a
/// normal app ships none of this.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use frust_core::{
        AnyView, BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent,
        LayoutCtx, PaintCtx, PaintScene, View, Widget, any,
    };
    use kurbo::{Affine, Point, Size};
    use peniko::Color;

    /// A leaf view of fixed intrinsic size that fills a rect on paint.
    pub struct Leaf {
        intrinsic: Size,
    }

    /// Build a [`Leaf`] with the given intrinsic width/height.
    pub fn leaf(width: f64, height: f64) -> Leaf {
        Leaf {
            intrinsic: Size::new(width, height),
        }
    }

    /// A [`Leaf`], type-erased for a `()`-state container.
    pub fn leaf_any(width: f64, height: f64) -> AnyView<()> {
        any(leaf(width, height))
    }

    impl Leaf {
        /// Erase this leaf into an `AnyView<()>`.
        pub fn into_any(self) -> AnyView<()> {
            any(self)
        }
    }

    /// Retained widget for [`Leaf`].
    pub struct LeafWidget {
        intrinsic: Size,
    }

    impl View<()> for Leaf {
        type Element = LeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
            LeafWidget {
                intrinsic: self.intrinsic,
            }
        }
        fn rebuild(
            &self,
            prev: &Self,
            element: &mut LeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.intrinsic != self.intrinsic {
                element.intrinsic = self.intrinsic;
                ChangeFlags::LAYOUT
            } else {
                ChangeFlags::NONE
            }
        }
    }

    impl Widget for LeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.intrinsic)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    /// A distinctive view whose widget always reports a 7x7 size — used to prove
    /// an `AnyView` type-swap actually replaced the widget.
    pub struct SwapLeaf;

    /// Build the [`SwapLeaf`] swap partner.
    pub fn swap_leaf() -> SwapLeaf {
        SwapLeaf
    }

    impl SwapLeaf {
        /// Erase this view into an `AnyView<()>`.
        pub fn into_any(self) -> AnyView<()> {
            any(self)
        }
    }

    /// Retained widget for [`SwapLeaf`].
    pub struct SwapLeafWidget;

    impl View<()> for SwapLeaf {
        type Element = SwapLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwapLeafWidget {
            SwapLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SwapLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for SwapLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(7.0, 7.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// A view whose widget fills its constraints and, on any event, records its
    /// `id` into the `Vec<u32>` application state and reports `Handled`. Used to
    /// prove event routing order.
    pub struct Probe {
        id: u32,
    }

    /// Build a [`Probe`] tagged with `id`.
    pub fn probe(id: u32) -> Probe {
        Probe { id }
    }

    impl Probe {
        /// Erase this probe into an `AnyView<Vec<u32>>`.
        pub fn into_any(self) -> AnyView<Vec<u32>> {
            any(self)
        }
    }

    /// Retained widget for [`Probe`].
    pub struct ProbeWidget {
        id: u32,
    }

    impl View<Vec<u32>> for Probe {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget { id: self.id }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            ctx.state_mut::<Vec<u32>>().push(self.id);
            ctx.request_redraw();
            EventResult::Handled
        }
    }

    /// A GPU-free [`PaintScene`] that records filled rects in paint order, plus
    /// `push_layer`/`push_transform` calls and their pop counts — the
    /// `motion::animated` wrappers' recording-scene tests assert against
    /// these alongside the pre-existing `rects`.
    #[derive(Default)]
    pub struct RecordingScene {
        pub rects: Vec<(Point, Size)>,
        pub layers: Vec<(Point, Size, f32)>,
        pub layer_pops: u32,
        pub transforms: Vec<Affine>,
        pub transform_pops: u32,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }
        fn pop_layer(&mut self) {
            self.layer_pops += 1;
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }
}
