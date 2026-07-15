//! Baseline widget set: Text, Button, Image, Column/Row, Stack, ScrollView, etc.
//! (spec §6.4).
//!
//! Ships the [`text`] leaf plus the spec §6.2 primitive layout containers —
//! [`Row`]/[`Column`] ([`FlexView`]), [`Stack`], [`Padding`], [`Align`], and
//! [`SizedBox`] — built as `View`/`Widget` pairs over `forgekit-core`'s
//! [`AnyView`](forgekit_core::AnyView)/[`ChildPod`](forgekit_core::ChildPod)
//! substrate. Containers own their children directly as `ChildPod`s (the arena
//! stays single-root); see [`forgekit_core::widget::ChildPod`] for the rationale.
//!
//! # Shared child plumbing
//!
//! The container modules build/rebuild/teardown their heterogeneous children
//! through the crate-private [`build_child`]/[`rebuild_child`]/[`teardown_child`]
//! helpers and route pointer events through [`route_event`]. Each child is an
//! [`AnyView`](forgekit_core::AnyView) whose element (`Box<dyn Widget>`) is stored
//! double-boxed inside a `ChildPod`, so a later rebuild can recover
//! `&mut Box<dyn Widget>` to drive `AnyView`'s type-erased reconciliation.

mod align;
mod button;
mod checkbox;
mod flex;
mod gesture;
mod padding;
mod scroll;
mod sized;
mod slider;
mod stack;
mod text;

use std::rc::Rc;

use forgekit_core::{
    AnyView, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, PointerPhase,
    View, Widget,
};

pub use align::{Align, AlignView, AlignWidget, Alignment};
pub use button::{Button, ButtonView, ButtonWidget, button};
pub use checkbox::{Checkbox, CheckboxView, CheckboxWidget, checkbox};
pub use flex::{
    Axis, Column, CrossAxisAlignment, FlexChild, FlexView, FlexWidget, MainAxisAlignment, Row,
    flexible, inflexible,
};
pub use gesture::{GestureDetector, GestureDetectorView, GestureDetectorWidget};
pub use padding::{EdgeInsets, Padding, PaddingView, PaddingWidget};
pub use scroll::{ScrollView, ScrollWidget, scroll_view};
pub use sized::{SizedBox, SizedBoxView, SizedBoxWidget};
pub use slider::{Slider, SliderView, SliderWidget, slider};
pub use stack::{Stack, StackView, StackWidget};
pub use text::{TextView, TextWidget, text};

/// A widget-held, `State`-erased app-state callback adapter (see
/// [`erase_callback`]).
pub(crate) type ErasedCallback = Box<dyn FnMut(&mut EventCtx)>;

/// A widget-held, `State`-erased callback adapter carrying one value argument
/// (see [`erase_callback_arg`]).
pub(crate) type ErasedArgCallback<A> = Box<dyn FnMut(&mut EventCtx, A)>;

/// A view-held, typed callback carrying one value argument (Checkbox's `bool`,
/// Slider's `f64`), erased to [`ErasedArgCallback`] on build.
pub(crate) type TypedArgCallback<State, A> = std::rc::Rc<dyn Fn(&mut State, A)>;

/// Erase a view-held `Rc<dyn Fn(&mut State)>` app-state callback into the
/// widget-held [`ErasedCallback`] adapter the interactive widgets invoke during
/// the event pass.
///
/// The closure recovers the concrete `State` from the type-erased [`EventCtx`]
/// with [`EventCtx::state_mut`] (the downcast happens *inside* the adapter), so
/// the widget itself stays non-generic over `State`. Closures aren't comparable,
/// so `build`/`rebuild` reinstall the adapter unconditionally — it's cheap.
pub(crate) fn erase_callback<State: 'static>(callback: &Rc<dyn Fn(&mut State)>) -> ErasedCallback {
    let callback = callback.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state);
    })
}

/// Like [`erase_callback`], but for callbacks that also carry a value argument
/// (Checkbox's `bool`, Slider's `f64`).
pub(crate) fn erase_callback_arg<State: 'static, A: 'static>(
    callback: &TypedArgCallback<State, A>,
) -> ErasedArgCallback<A> {
    let callback = callback.clone();
    Box::new(move |ctx: &mut EventCtx, arg: A| {
        let state = ctx.state_mut::<State>();
        callback(state, arg);
    })
}

/// Build a [`ChildPod`] wrapping an [`AnyView`]'s element.
///
/// The element (`Box<dyn Widget>`) is stored double-boxed so a later
/// [`rebuild_child`] can recover it as `&mut Box<dyn Widget>` — the type
/// `AnyView`'s `rebuild` needs to swap the widget on a concrete-type change.
pub(crate) fn build_child<State: 'static>(
    view: &AnyView<State>,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

/// Reconcile one [`AnyView`] child in place through its `ChildPod`.
pub(crate) fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("layout-container child element is a boxed AnyView widget");
    next.rebuild(prev, element, ctx)
}

/// Tear down one [`AnyView`] child through its `ChildPod`.
pub(crate) fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

/// Positionally diff a homogeneous `Vec<AnyView>` against its live `ChildPod`s.
///
/// Common indices rebuild in place; a grown tail is built; a shrunk tail is torn
/// down and dropped. Length changes signal `LAYOUT | PAINT`.
pub(crate) fn rebuild_children<State: 'static>(
    prev: &[AnyView<State>],
    next: &[AnyView<State>],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let mut flags = ChangeFlags::NONE;
    let common = prev.len().min(next.len());
    for i in 0..common {
        flags |= rebuild_child(&prev[i], &next[i], &mut pods[i], ctx);
    }
    if next.len() > prev.len() {
        for view in &next[prev.len()..] {
            pods.push(build_child(view, ctx));
        }
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    } else if next.len() < prev.len() {
        for (offset, view) in prev[next.len()..].iter().enumerate() {
            teardown_child(view, &mut pods[next.len() + offset], ctx);
        }
        pods.truncate(next.len());
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    flags
}

/// Route a pointer/scroll event to a container's children.
///
/// A captured gesture goes straight to the recorded active child (capture
/// auto-releases on `Up`/`Cancel`). Otherwise the children are hit-tested in
/// reverse paint order — topmost (last-painted) first — and the first child that
/// both contains the point and reports [`EventResult::Handled`] consumes it. This
/// is the z-order convention Flex establishes and Stack shares.
pub(crate) fn route_event(
    children: &mut [ChildPod],
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    let releasing = matches!(
        event,
        InputEvent::Pointer(p)
            if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
    );
    if let Some(pod) = children.iter_mut().find(|p| p.is_active()) {
        let result = pod.event_child(ctx, event);
        if releasing {
            pod.set_active(false);
        }
        return result;
    }
    let position = event.position();
    for pod in children.iter_mut().rev() {
        if pod.contains(position) && pod.event_child(ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
    }
    EventResult::Ignored
}

/// Shared, GPU-free fixtures for the container layout/paint/event tests: a
/// fixed-size [`Leaf`], a distinctive swap partner, an event-recording
/// [`Probe`], and a recording [`RecordingScene`].
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use forgekit_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, any};
    use kurbo::{Point, Size};
    use peniko::Color;

    /// A leaf view of fixed intrinsic size that fills a rect on paint.
    pub(crate) struct Leaf {
        intrinsic: Size,
    }

    /// Build a [`Leaf`] with the given intrinsic width/height.
    pub(crate) fn leaf(width: f64, height: f64) -> Leaf {
        Leaf {
            intrinsic: Size::new(width, height),
        }
    }

    /// A [`Leaf`], type-erased for a `()`-state container.
    pub(crate) fn leaf_any(width: f64, height: f64) -> AnyView<()> {
        any(leaf(width, height))
    }

    impl Leaf {
        /// Erase this leaf into an `AnyView<()>`.
        pub(crate) fn into_any(self) -> AnyView<()> {
            any(self)
        }
    }

    /// Retained widget for [`Leaf`].
    pub(crate) struct LeafWidget {
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
    pub(crate) struct SwapLeaf;

    /// Build the [`SwapLeaf`] swap partner.
    pub(crate) fn swap_leaf() -> SwapLeaf {
        SwapLeaf
    }

    impl SwapLeaf {
        /// Erase this view into an `AnyView<()>`.
        pub(crate) fn into_any(self) -> AnyView<()> {
            any(self)
        }
    }

    /// Retained widget for [`SwapLeaf`].
    pub(crate) struct SwapLeafWidget;

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
    pub(crate) struct Probe {
        id: u32,
    }

    /// Build a [`Probe`] tagged with `id`.
    pub(crate) fn probe(id: u32) -> Probe {
        Probe { id }
    }

    impl Probe {
        /// Erase this probe into an `AnyView<Vec<u32>>`.
        pub(crate) fn into_any(self) -> AnyView<Vec<u32>> {
            any(self)
        }
    }

    /// Retained widget for [`Probe`].
    pub(crate) struct ProbeWidget {
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

    /// A GPU-free [`PaintScene`] that records filled rects in paint order.
    #[derive(Default)]
    pub(crate) struct RecordingScene {
        pub(crate) rects: Vec<(Point, Size)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }
}
