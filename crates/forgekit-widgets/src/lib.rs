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
//! helpers and route pointer events through [`route_event`] (multi-child
//! containers — `Flex`/`Stack`) or [`route_event_single`] (one-child wrappers —
//! `Padding`/`Align`/`SizedBox`). Each child is an
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

use std::any::Any;
use std::rc::Rc;

use forgekit_core::{
    AnyView, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, PointerButton,
    PointerEvent, PointerPhase, View, Widget,
};
use kurbo::Point;

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
    let (flags, _swapped) = rebuild_child_tracked(prev, next, pod, ctx);
    flags
}

/// Like [`rebuild_child`], but also reports whether the rebuild *replaced* the
/// underlying widget (an [`AnyView`] concrete-type swap) rather than mutating it
/// in place.
///
/// The swap flag drives [`rebuild_children`]'s capture bookkeeping: a fresh
/// widget swapped in at a still-captured index never saw the original `Down`, so
/// its stale `active` path is dropped without a synthetic `Cancel` (there is
/// nothing armed to unwind). Detection compares the boxed element's concrete
/// [`TypeId`](std::any::TypeId) across the rebuild.
fn rebuild_child_tracked<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> (ChangeFlags, bool) {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("layout-container child element is a boxed AnyView widget");
    let before = {
        let any: &dyn Any = &**element;
        any.type_id()
    };
    let flags = next.rebuild(prev, element, ctx);
    let after = {
        let any: &dyn Any = &**element;
        any.type_id()
    };
    (flags, before != after)
}

/// Tear down one [`AnyView`] child through its `ChildPod`.
///
/// A pod still holding an in-flight capture ([`ChildPod::is_active`]) is
/// [cancelled](cancel_pod) before teardown, so an armed widget dropped mid-gesture
/// (e.g. the active row truncated out of a shrinking list) unwinds its state
/// machine instead of vanishing with no terminating `Up`/`Cancel`.
pub(crate) fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if pod.is_active() {
        cancel_pod(pod);
        pod.set_active(false);
    }
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

/// Deliver a synthetic [`PointerPhase::Cancel`] to a captured child whose
/// in-flight gesture a structural rebuild has invalidated, so its
/// (g2-hardened) state machine unwinds instead of firing on a later `Up`.
///
/// # Cancel-during-rebuild contract
///
/// The rebuild pass runs over a [`BuildCtx`], not an [`EventCtx`] — there is no
/// application state in scope. This is sound *only because a `Cancel` handler
/// must never read application state* (`EventCtx::state_mut`): post-g2 every
/// interactive widget's `Cancel` arm only clears internal flags. That invariant
/// lets this build a minimal [`EventCtx`] over a throwaway `()` state to drive
/// the unwind; a `Cancel` handler that reached for real state would panic here
/// on the `()` downcast — a deliberate tripwire, not a silent corruption.
fn cancel_pod(pod: &mut ChildPod) {
    let mut dummy_state = ();
    let mut ctx = EventCtx::new(&mut dummy_state, pod.origin(), pod.size());
    // Position is irrelevant to a `Cancel` (handlers never hit-test on it).
    let cancel = InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Cancel,
        position: Point::ZERO,
        button: PointerButton::Primary,
    });
    pod.event_child(&mut ctx, &cancel);
}

/// Cancel and clear every child still holding a recorded active (captured) path.
///
/// Called by [`rebuild_children`] once a structural change is detected: the
/// recorded path can no longer be trusted, so any surviving armed widget is
/// unwound via [`cancel_pod`] and its `active` flag dropped.
fn cancel_active_children(pods: &mut [ChildPod]) {
    for pod in pods.iter_mut() {
        if pod.is_active() {
            cancel_pod(pod);
            pod.set_active(false);
        }
    }
}

/// Positionally diff a child list against its live `ChildPod`s, extracting each
/// child's [`AnyView`] through `view_of` (identity for a plain `Vec<AnyView>`,
/// `|c| &c.view` for Flex's `FlexChild` wrapper — the one shared reconciler).
///
/// Common indices rebuild in place; a grown tail is built; a shrunk tail is torn
/// down and dropped. Length changes signal `LAYOUT | PAINT`.
///
/// **Structural change cancels in-flight gestures.** Positional reconciliation
/// misroutes a captured gesture across a structural edit: a length shift moves
/// the armed widget to a different logical index, and a type swap replaces the
/// widget under a still-recorded path. On any child-count change *or* in-place
/// type swap, every surviving captured child is [cancelled](cancel_active_children)
/// so it unwinds rather than firing on a hit-tested `Up`; a swapped-in fresh
/// widget (which never saw `Down`) just has its stale path dropped; and a
/// truncated active pod is cancelled in [`teardown_child`]. A structural-change-free
/// rebuild leaves the recorded path untouched, so an ordinary every-frame rebuild
/// never breaks a captured drag.
pub(crate) fn rebuild_children<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
) -> ChangeFlags {
    let mut flags = ChangeFlags::NONE;
    let common = prev.len().min(next.len());
    let mut swapped = false;
    for i in 0..common {
        let (child_flags, child_swapped) =
            rebuild_child_tracked(view_of(&prev[i]), view_of(&next[i]), &mut pods[i], ctx);
        flags |= child_flags;
        if child_swapped {
            // Fresh widget at this slot: drop the stale capture, nothing to cancel.
            pods[i].set_active(false);
            swapped = true;
        }
    }
    if next.len() > prev.len() {
        for child in &next[prev.len()..] {
            pods.push(build_child(view_of(child), ctx));
        }
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    } else if next.len() < prev.len() {
        for (offset, child) in prev[next.len()..].iter().enumerate() {
            teardown_child(view_of(child), &mut pods[next.len() + offset], ctx);
        }
        pods.truncate(next.len());
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    // A structural change — child-count shift or an in-place type swap —
    // invalidates any surviving in-flight capture (truncated active pods were
    // already cancelled in teardown; swapped slots were cleared above).
    if prev.len() != next.len() || swapped {
        cancel_active_children(pods);
    }
    flags
}

/// Whether `event` is the phase that auto-releases a recorded capture
/// (`Up`/`Cancel`) — shared by [`route_event`]/[`route_event_single`].
fn releases_capture(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p)
            if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
    )
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
    if let Some(pod) = children.iter_mut().find(|p| p.is_active()) {
        return route_event_single(pod, ctx, event);
    }
    let position = event.position();
    for pod in children.iter_mut().rev() {
        if pod.contains(position) && pod.event_child(ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
    }
    EventResult::Ignored
}

/// Route a pointer/scroll event to a container's single child.
///
/// Mirrors [`route_event`] for the one-child wrappers (`Padding`/`Align`/
/// `SizedBox`): a captured gesture is forwarded to `pod` unconditionally —
/// regardless of whether the event's position still falls within the child's
/// bounds — with the active path cleared on `Up`/`Cancel`; otherwise the child
/// only receives the event if it contains the point. Re-hit-testing
/// `pod.contains()` on every event instead of consulting [`ChildPod::is_active`]
/// is the bug this helper exists to prevent — see `ChildPod::contains`'s docs.
pub(crate) fn route_event_single(
    pod: &mut ChildPod,
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if pod.is_active() {
        let result = pod.event_child(ctx, event);
        if releases_capture(event) {
            pod.set_active(false);
        }
        return result;
    }
    if pod.contains(event.position()) {
        pod.event_child(ctx, event)
    } else {
        EventResult::Ignored
    }
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
