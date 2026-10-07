//! Component: Flutter's `StatefulWidget` analog.
//!
//! A [`Component`] is a piece of UI with *retained local state* that lives in
//! the widget tree, a per-component reactive [`Owner`] (context scoping +
//! `on_cleanup`), and a state boundary the surrounding view tree never sees. It
//! is the seam that lets a subtree own state independent of the ambient
//! application state: a [`ComponentView`] implements `View<Outer>` for **any**
//! outer state, and the subtree it hosts is diffed against the component's own
//! `State` instead.
//!
//! # The state boundary
//!
//! [`ComponentWidget`] builds an inner [`EventCtx`] over its own
//! `C::State` and dispatches events to its child through the same
//! capture/focus/IME routing contract single-child containers use (mirroring
//! `frust-widgets`' `route_event_single`), then mirrors the inner results
//! (redraw / capture / focus request-release / IME publish) back onto the
//! *outer* context. The subtree therefore mutates the component's local state
//! while the outer tree only observes the effects (a redraw, a capture, a focus
//! change) — exactly as a container observes them from a leaf.
//!
//! # Reactive ownership
//!
//! Each component owns a child [`Owner`] created under the ambient owner (the
//! root `Owner` a shell wraps its rebuild in; a nested component inherits its
//! parent component's owner because build/rebuild run under `owner.with`). The
//! owner scopes `provide_context`/`use_context` and `on_cleanup`, and is
//! disposed explicitly at teardown (and defensively on `Drop`).
//!
//! # What `Component` does NOT do (yet)
//!
//! Per-component tracked/reactive scopes and rebuild skipping are not
//! implemented: a component **always** re-runs `build` on rebuild because its
//! local state may have changed even when the component value compares equal.
//! Root-level components are never torn down (the root widget lives for the
//! app's life), so a root component's `on_cleanup`/owner disposal never runs —
//! a known, accepted limitation.

use std::any::Any;

use kurbo::{Point, Size};
use reactive_graph::owner::Owner;

use crate::event::{EventCtx, EventResult, InputEvent, PointerPhase};
#[cfg(feature = "hotpatch")]
use crate::hotpatch::SeamWitness;
use crate::layout::BoxConstraints;
use crate::view::{AnyView, BuildCtx, ChangeFlags, View};
use crate::widget::{ChildPod, LayoutCtx, PaintCtx, PaintScene, Widget};

/// A piece of UI with retained local state (Flutter's `StatefulWidget` analog).
///
/// A `Component` declares an associated [`Component::State`] type, seeds it once
/// with [`Component::init`], and produces its subtree with [`Component::build`],
/// which receives `&mut State` so the build can read (and the subtree's events
/// can mutate) the retained state. The subtree is any `impl View` over the
/// component's own `State` — the outer state type never appears. Erasure
/// happens at the API boundary: [`ComponentWidget`] wraps whatever `build`
/// returns in an [`AnyView`] before storing it, and because [`AnyView::new`] is
/// idempotent a body that already returns an `AnyView` (e.g. `any(..)`) is not
/// boxed a second time.
pub trait Component: 'static {
    /// The retained local state this component owns across rebuilds.
    type State: 'static;

    /// Create the component's initial local state. Runs exactly once, when the
    /// [`ComponentWidget`] is first built, under the component's [`Owner`] (so
    /// `on_cleanup`/`provide_context` registered here bind to that owner).
    ///
    /// A *root* component (the one an entry macro or `frust::run` drives) has
    /// no enclosing [`ComponentWidget`] and so no per-component owner of its
    /// own: the shell instead runs its `init`/`build` under the shell's **root**
    /// [`Owner`], which lives for the whole process and is never disposed. Its
    /// `on_cleanup`/`provide_context` therefore bind to that root owner — the
    /// context is visible to the entire tree, and cleanups run at process exit
    /// rather than on teardown (there is no teardown for the root).
    fn init(&self) -> Self::State;

    /// Produce the component's subtree from its current local state.
    ///
    /// Re-run on every rebuild (local state may have changed even when the
    /// component value is equal). The returned view is erased into an
    /// [`AnyView`] by the hosting [`ComponentWidget`] and diffed against the
    /// previous one exactly like `RenderRoot`'s root view is. Implementations
    /// declare the return type as `impl View<Self::State>`; returning a concrete
    /// view type or an `any(..)` value both satisfy it.
    ///
    /// The opaque return type captures the `&self` and `&mut State` borrows, so
    /// a caller that hands the result out of a closure (a root driver's
    /// `move |state| ..` build closure) erases it first with
    /// `AnyView::new(root.build(state))`.
    fn build(&self, state: &mut Self::State) -> impl View<Self::State>;
}

/// The `View` adapter that hosts a [`Component`] in any surrounding view tree.
///
/// `ComponentView<C>` implements `View<Outer>` for **every** `Outer` state:
/// the hosted subtree is bound to `C::State`, not `Outer`, so a component can
/// be dropped anywhere regardless of the ambient application state. Build it
/// with the [`component`] free function.
pub struct ComponentView<C: Component> {
    component: C,
}

/// Host a [`Component`] as a [`View`] — the view-fn spelling of
/// [`ComponentView`], mirroring the `text(..)`/`button(..)` vocabulary.
pub fn component<C: Component>(c: C) -> ComponentView<C> {
    ComponentView { component: c }
}

/// The retained widget behind a [`ComponentView`].
///
/// Owns the component's local `State`, the previous child [`AnyView`] (diffed
/// each rebuild), the child element wrapped in a [`ChildPod`], the component's
/// reactive [`Owner`], and a component-local [`BuildCtx`] id counter.
///
/// The child element is stored **double-boxed** inside the pod (`ChildPod::new(
/// Box::new(element))`, where `element: Box<dyn Widget>` is the [`AnyView`]'s
/// element): the extra box lets a rebuild recover the child as
/// `&mut Box<dyn Widget>`, the exact type [`AnyView::rebuild`] needs to swap the
/// widget on a concrete-type change. This mirrors `frust-widgets`'
/// `build_child`/`rebuild_child` pod convention.
///
/// # Under the `hotpatch` feature
///
/// The struct is `repr(C)`, starts with the [`SeamWitness`] the creating image
/// wrote, and holds the state boxed, so every field sits at the same offset in
/// every image whatever `C::State` looks like there. `rebuild`, `teardown` and
/// `Drop` check the witness first; on a mismatch nothing `C`-generic touches the
/// state, and `Drop` leaks the state box (`mem::forget`) rather than let another
/// image's layout drop or free it, while still disposing the owner.
#[cfg_attr(feature = "hotpatch", repr(C))]
pub struct ComponentWidget<C: Component> {
    /// The layout of `C` and `C::State` in the image that built this widget.
    #[cfg(feature = "hotpatch")]
    witness: SeamWitness,
    /// The component's retained local state; the inner [`EventCtx`] is built
    /// over this, so the subtree's event handlers mutate it via `state_mut`.
    /// Always `Some` while the widget is live; `Drop` takes it to leak it on a
    /// layout mismatch.
    #[cfg(feature = "hotpatch")]
    state: Option<Box<C::State>>,
    /// The component's retained local state; the inner [`EventCtx`] is built
    /// over this, so the subtree's event handlers mutate it via `state_mut`.
    #[cfg(not(feature = "hotpatch"))]
    state: C::State,
    /// The previous child view, diffed against the freshly built one each
    /// rebuild (the same reconcile `RenderRoot::rebuild_view` performs).
    prev: AnyView<C::State>,
    /// The child element (a double-boxed [`AnyView`] element) plus its geometry
    /// and capture/focus bookkeeping.
    child: ChildPod,
    /// The component's reactive owner — child of the ambient owner, scoping
    /// context and `on_cleanup`. Disposed explicitly at teardown / drop.
    owner: Owner,
    /// Component-local widget-id counter for the child's [`BuildCtx`] (decoupled
    /// from the arena's monotonic ids because pod-owned children never enter the
    /// arena).
    next_id: u64,
    /// Whether the owner has already been disposed, so teardown + `Drop` never
    /// run cleanups twice.
    disposed: bool,
}

impl<C: Component> ComponentWidget<C> {
    /// Dispose the component's owner exactly once, running its `on_cleanup`s and
    /// dropping any arena-allocated reactive values. Idempotent: teardown and a
    /// later defensive `Drop` both call this, but the cleanups fire once.
    fn dispose(&mut self) {
        if !self.disposed {
            // `Owner::cleanup` runs this owner's (and its children's) registered
            // `on_cleanup`s and drops its arena nodes; it drains those lists, so
            // a second call — including the `Drop` fallback below — is a no-op.
            self.owner.cleanup();
            self.disposed = true;
        }
    }

    /// The retained local state.
    #[cfg(feature = "hotpatch")]
    fn state_mut(&mut self) -> &mut C::State {
        self.state
            .as_deref_mut()
            .expect("component state is present while the widget is live")
    }

    /// The retained local state.
    #[cfg(not(feature = "hotpatch"))]
    fn state_mut(&mut self) -> &mut C::State {
        &mut self.state
    }

    /// The retained local state, read-only.
    #[cfg(all(test, feature = "hotpatch"))]
    fn state(&self) -> &C::State {
        self.state
            .as_deref()
            .expect("component state is present while the widget is live")
    }

    /// The retained local state, read-only.
    #[cfg(all(test, not(feature = "hotpatch")))]
    fn state(&self) -> &C::State {
        &self.state
    }

    /// Overwrite the stored witness, standing in for a widget another image
    /// built.
    #[cfg(all(test, feature = "hotpatch"))]
    fn set_witness_for_test(&mut self, witness: SeamWitness) {
        self.witness = witness;
    }
}

impl<C: Component> Drop for ComponentWidget<C> {
    fn drop(&mut self) {
        // A mismatched witness means this drop glue belongs to an image whose
        // `C::State` layout differs from the creator's: leak the state box
        // without dropping or freeing it. `take` reads and clears only the
        // pointer, whose layout does not depend on `C::State`.
        #[cfg(feature = "hotpatch")]
        if crate::hotpatch::verify_witness::<C>(self.witness).is_err() {
            std::mem::forget(self.state.take());
        }
        // Defensive: a component removed by a path that did not route through
        // `View::teardown` (or a panic mid-teardown) still disposes its owner.
        self.dispose();
    }
}

impl<Outer: 'static, C: Component> View<Outer> for ComponentView<C> {
    type Element = ComponentWidget<C>;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
        // A child of the ambient owner: the root owner a shell installs, or the
        // enclosing component's owner (we build under `owner.with`, below).
        let owner = Owner::new();
        // This image runs `init`, so its layout is the creator's.
        #[cfg(feature = "hotpatch")]
        let witness = SeamWitness::of::<C>();
        let (state, prev, child, next_id) = owner.with(|| {
            #[cfg(feature = "hotpatch")]
            let mut state = Box::new(self.component.init());
            #[cfg(not(feature = "hotpatch"))]
            let mut state = self.component.init();
            // Erase at the boundary; idempotent, so an `AnyView` body is not
            // re-boxed. The seam erases inside the hot function; a patch that
            // already changed `C`'s layout yields an inert child instead.
            #[cfg(feature = "hotpatch")]
            let view = crate::hotpatch::built_or_inert(crate::hotpatch::build_erased(
                &self.component,
                &mut *state,
                witness,
            ));
            #[cfg(not(feature = "hotpatch"))]
            let view = AnyView::new(self.component.build(&mut state));
            // A component-local build counter; the child never enters the arena.
            let mut next_id = 0u64;
            let mut inner = BuildCtx::new(&mut next_id);
            // A freshly built child pod holds no focus link (`ChildPod::new`
            // starts unfocused), so the chain across this boundary is closed —
            // and a build tears nothing down, so nothing under it can orphan a
            // session (see `BuildCtx::has_focus`).
            inner.set_has_focus(false);
            let element: Box<dyn Widget> = view.build(&mut inner);
            // Double-box so a later rebuild can recover `&mut Box<dyn Widget>`.
            (state, view, ChildPod::new(Box::new(element)), next_id)
        });
        ComponentWidget {
            #[cfg(feature = "hotpatch")]
            witness,
            #[cfg(feature = "hotpatch")]
            state: Some(state),
            #[cfg(not(feature = "hotpatch"))]
            state,
            prev,
            child,
            owner,
            next_id,
            disposed: false,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Before anything forms `&mut C::State` or reads `prev`/`child`: a
        // widget another image built with a different layout is left exactly as
        // it is (the stale subtree stays until the session restarts).
        #[cfg(feature = "hotpatch")]
        if crate::hotpatch::verify_witness::<C>(element.witness).is_err() {
            return ChangeFlags::NONE;
        }
        // The outer `_prev` ComponentView is intentionally unused: the component
        // always re-runs `build` because its retained local state may have
        // changed even when the component value is equal (tracked-scope skipping
        // is not implemented).
        //
        // The outer `ctx`, by contrast, is read for exactly one thing: the
        // effective focus chain, which must cross this state boundary or every
        // reconciler under a component would start a fresh (assumed-live) chain.
        // Read here because the inner context is built inside the `owner.with`
        // closure below, which cannot also borrow `ctx`.
        let outer_has_focus = ctx.has_focus();
        let owner = element.owner.clone();
        owner.with(|| {
            // The seam passes the stored witness, so a patched hot function
            // compares the creator's layout with its own.
            #[cfg(feature = "hotpatch")]
            let new_view = {
                let witness = element.witness;
                match crate::hotpatch::build_erased(&self.component, element.state_mut(), witness) {
                    Ok(view) => view,
                    // Reported by the seam; nothing below has been touched.
                    Err(_) => return ChangeFlags::NONE,
                }
            };
            #[cfg(not(feature = "hotpatch"))]
            let new_view = AnyView::new(self.component.build(element.state_mut()));
            // Read before the `widget_mut` borrow below.
            let child_focused = element.child.is_focused();
            let (flags, swapped) = {
                let boxed = element
                    .child
                    .widget_mut()
                    .downcast_mut::<Box<dyn Widget>>()
                    .expect("component child element is a boxed AnyView widget");
                let before = {
                    let any: &dyn Any = &**boxed;
                    any.type_id()
                };
                let mut inner = BuildCtx::new(&mut element.next_id);
                // Extend the chain by this component's own child link — the
                // `ChildPod::paint_child` composition, spelled by hand because
                // the component descends into its pod through a fresh inner
                // context rather than through the pod plumbing.
                inner.set_has_focus(outer_has_focus && child_focused);
                let flags = new_view.rebuild(&element.prev, boxed, &mut inner);
                let after = {
                    let any: &dyn Any = &**boxed;
                    any.type_id()
                };
                (flags, before != after)
            };
            if swapped {
                // The child AnyView swapped concrete type: the old widget was
                // torn down inside `AnyView::rebuild`, so any capture/focus path
                // it recorded is stale — drop it (mirrors `rebuild_child`).
                element.child.set_active(false);
                element.child.set_focused(false);
                // Dropping the flag is not the load-bearing part: the focused
                // widget just stopped existing, so a LIVE session died with it
                // and the root must release it before the frame ends. Gated on
                // the whole chain — `outer_has_focus && child_focused` is the
                // same composition `teardown` spells below and the same one
                // `frust-widgets`' `mark_orphan_if_live` applies at its own
                // marking sites (this crate sits below that one, so the gate is
                // spelled by hand rather than shared).
                if outer_has_focus && child_focused {
                    crate::event::mark_focus_orphaned();
                }
            }
            element.prev = new_view;
            flags
        })
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        // A widget another image built with a different layout is not torn
        // down: neither the child teardown through `prev` nor the dispose runs
        // here (`Drop` disposes the owner and leaks the state).
        #[cfg(feature = "hotpatch")]
        if crate::hotpatch::verify_witness::<C>(element.witness).is_err() {
            return;
        }
        // Forward teardown to the child view/element under the component owner,
        // then dispose the owner (runs `on_cleanup`s) and let the state drop.
        //
        // The outer focus chain crosses the boundary here exactly as it does in
        // `rebuild`: a component torn down *inside a live focus chain* must let
        // the reconcilers below it mark the orphan, and one torn down under a
        // cleared link must not (see `BuildCtx::has_focus`).
        let inner_has_focus = ctx.has_focus() && element.child.is_focused();
        element.owner.clone().with(|| {
            let mut next_id = element.next_id;
            let mut inner = BuildCtx::new(&mut next_id);
            inner.set_has_focus(inner_has_focus);
            if let Some(boxed) = element.child.widget_mut().downcast_mut::<Box<dyn Widget>>() {
                element.prev.teardown(boxed, &mut inner);
            }
        });
        element.dispose();
    }
}

impl<C: Component> Widget for ComponentWidget<C> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Pass-through: the component contributes no chrome of its own. The
        // child sits at the component's origin (ZERO in the component's local
        // space) and receives the incoming constraints unchanged.
        self.child.set_origin(Point::ZERO);
        self.child.layout_child(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Pass-through to the pod at origin ZERO: `paint_child` offsets by the
        // incoming `ctx.origin()`, bubbles `needs_frame`, republishes a focused
        // editable's IME surface, and composes `has_focus` — exactly like a
        // single-child container.
        self.child.paint_child(ctx, scene);
    }

    fn semantics(&self, ctx: &mut crate::semantics::SemanticsCtx) {
        // A component contributes no node of its own (the state boundary is
        // invisible to accessibility); forward to the child pod, exactly like a
        // transparent single-child container.
        self.child.semantics_child(ctx);
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        // The component boundary is invisible to tooling for the same reason it
        // is invisible to accessibility: it contributes no element of its own,
        // only the child it wraps.
        visitor(&self.child);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The state boundary: build an inner context over the component's *own*
        // local state, seeded with the outer focus flag, dispatch through the
        // pod with the single-child routing contract, then mirror the inner
        // results back onto the outer context.
        //
        // A synthesized `Cancel` (from an outer structural rebuild) arrives over
        // throwaway `()` outer state, but this handler never touches outer state
        // — it forwards to the subtree over the component's real state, safe
        // under the Cancel-never-reads-state contract (which binds the subtree's
        // handlers, not this carrier).
        let size = ctx.size();
        let has_focus = ctx.has_focus();
        // The hover link, its live epoch, and this pass's claim eligibility all
        // cross the state boundary unchanged — a component is transparent to hover
        // exactly as it is to focus, so a claim made below it still reaches the root
        // and stamps the pod chain on the way (mirrored back through `claim_hover`).
        let hovered = ctx.is_hovered();
        let hover_epoch = ctx.hover_epoch();
        let hover_eligible = ctx.is_hover_eligible();
        // A cursor request needs no threading here at all, unlike every other
        // channel this carrier mirrors: it rides one pass-scoped slot rather than a
        // per-context field (see `crate::event`'s `CURSOR_REQUEST`), so a
        // `set_cursor` below this boundary already reaches the root, and the
        // component boundary is transparent to it by construction.
        let (result, needs_redraw, captured, hover_claimed, focus_req, focus_rel, ime) = {
            // Spelled per field (not through `state_mut`) so the borrow stays
            // disjoint from `self.child` below.
            #[cfg(feature = "hotpatch")]
            let state_any: &mut dyn Any = self
                .state
                .as_deref_mut()
                .expect("component state is present while the widget is live");
            #[cfg(not(feature = "hotpatch"))]
            let state_any: &mut dyn Any = &mut self.state;
            let mut inner = EventCtx::new(state_any, Point::ZERO, size);
            inner.set_has_focus(has_focus);
            inner.set_hovered(hovered);
            inner.set_hover_epoch(hover_epoch);
            inner.set_hover_eligible(hover_eligible);
            let result = route_child(&mut self.child, &mut inner, event);
            (
                result,
                inner.needs_redraw(),
                inner.is_pointer_captured(),
                inner.is_hover_claimed(),
                inner.is_focus_requested(),
                inner.is_focus_released(),
                inner.take_ime_state(),
            )
        };
        if needs_redraw {
            ctx.request_redraw();
        }
        if captured {
            ctx.capture_pointer();
        }
        if hover_claimed {
            ctx.claim_hover();
        }
        if focus_req {
            ctx.request_focus();
        }
        if focus_rel {
            ctx.release_focus();
        }
        if let Some(ime) = ime {
            ctx.publish_ime_state(ime);
        }
        result
    }
}

/// Route an event to the component's single child pod with the recorded-path
/// semantics single-child containers use (the in-crate mirror of
/// `frust-widgets`' `route_event_single`).
///
/// A broadcast ([`InputEvent::is_broadcast`]) reaches the child unconditionally,
/// checked **first** so no capture/focus/hit-test branch below can swallow it —
/// a component sitting between the root and a navigator is the ordinary shape,
/// so this arm is what lets a deferred pop-result flush reach that navigator at
/// all. A focus-routed event goes to the child only if it holds the recorded
/// focus path; a captured (active) child receives pointer events unconditionally
/// (capture auto-releasing on `Up`/`Cancel`), otherwise the child receives the
/// event only if it contains the point, and a `Down` that misses a focused
/// child blurs it.
fn route_child(pod: &mut ChildPod, ctx: &mut EventCtx<'_>, event: &InputEvent) -> EventResult {
    if event.is_broadcast() {
        // Never consumed: forward, discard the result (see the variant's
        // routing contract).
        pod.event_child(ctx, event);
        return EventResult::Ignored;
    }
    if event.is_focus_routed() {
        if pod.is_focused() {
            return pod.event_child(ctx, event);
        }
        return EventResult::Ignored;
    }
    if pod.is_active() {
        let result = pod.event_child(ctx, event);
        if releases_capture(event) {
            pod.set_active(false);
        }
        return result;
    }
    let inside = pod.contains(event.position());
    let result = if inside {
        pod.event_child(ctx, event)
    } else {
        EventResult::Ignored
    };
    if is_pointer_down(event) && !inside && pod.is_focused() {
        pod.set_focused(false);
    }
    result
}

/// Whether `event` is the phase that auto-releases a recorded capture
/// (`Up`/`Cancel`).
fn releases_capture(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
    )
}

/// Whether `event` is a pointer `Down` (opens capture / triggers blur eval).
fn is_pointer_down(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Down)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        CursorIcon, EditingState, ImeState, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
        PointerEvent, clear_cursor_request, take_cursor_request,
    };
    use crate::view::any;
    use kurbo::Rect;
    use reactive_graph::owner::{on_cleanup, provide_context, use_context};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    // --- Event constructors -------------------------------------------------

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key_enter() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    // --- A trivial leaf view/widget used to fill component subtrees ---------

    struct Empty;
    struct EmptyWidget;
    impl Widget for EmptyWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }
    impl View<()> for Empty {
        type Element = EmptyWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> EmptyWidget {
            EmptyWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut EmptyWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    /// A second, distinct leaf type — an `AnyView` swap partner for `Empty`.
    struct OtherLeaf;
    struct OtherWidget;
    impl Widget for OtherWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }
    impl View<()> for OtherLeaf {
        type Element = OtherWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> OtherWidget {
            OtherWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut OtherWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    // --- Criterion 1: local-state retention (a click counter) ---------------

    struct CounterState {
        count: u32,
    }

    /// A component whose button increments its own local `count`. Carries a
    /// `label` prop so a parent-driven rebuild can change props without
    /// disturbing the retained state.
    struct ClickCounter {
        label: &'static str,
    }

    impl Component for ClickCounter {
        type State = CounterState;
        fn init(&self) -> CounterState {
            CounterState { count: 0 }
        }
        fn build(&self, state: &mut CounterState) -> impl View<CounterState> {
            any(CounterButtonView {
                count: state.count,
                label: self.label,
            })
        }
    }

    struct CounterButtonView {
        count: u32,
        label: &'static str,
    }
    struct CounterButtonWidget {
        count: u32,
        label: &'static str,
    }
    impl View<CounterState> for CounterButtonView {
        type Element = CounterButtonWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterButtonWidget {
            CounterButtonWidget {
                count: self.count,
                label: self.label,
            }
        }
        fn rebuild(
            &self,
            prev: &Self,
            element: &mut CounterButtonWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            let mut flags = ChangeFlags::NONE;
            if prev.count != self.count || prev.label != self.label {
                element.count = self.count;
                element.label = self.label;
                flags = ChangeFlags::PAINT;
            }
            flags
        }
    }
    impl Widget for CounterButtonWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 20.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.state_mut::<CounterState>().count += 1;
                ctx.request_redraw();
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    /// Set (and keep alive) an ambient owner for a test, so components created
    /// under it become children of a real owner (context/cleanup work).
    fn ambient() -> Owner {
        let owner = Owner::new();
        owner.set();
        owner
    }

    fn build_widget<Outer: 'static, C: Component>(view: &ComponentView<C>) -> ComponentWidget<C> {
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<Outer>::build(view, &mut ctx)
    }

    #[test]
    fn local_state_survives_parent_driven_rebuild() {
        let _owner = ambient();
        let v1 = component(ClickCounter { label: "a" });
        let mut widget = build_widget::<(), _>(&v1);
        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::tight(Size::new(40.0, 20.0)));

        // Three clicks land inside the button and increment the component's own
        // local state through the inner EventCtx.
        let mut outer = ();
        for _ in 0..3 {
            let mut ectx = EventCtx::new(&mut outer, Point::ZERO, Size::new(40.0, 20.0));
            widget.event(&mut ectx, &pointer(PointerPhase::Down, 5.0, 5.0));
        }
        assert_eq!(widget.state().count, 3);

        // A parent-driven rebuild with changed props must not reset the count.
        let v2 = component(ClickCounter { label: "changed" });
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::rebuild(&v2, &v1, &mut widget, &mut ctx);
        assert_eq!(
            widget.state().count,
            3,
            "local state retained across rebuild"
        );
    }

    // --- Criterion 2: context scoping across the component boundary ---------

    #[derive(Clone)]
    struct Theme(i32);

    /// Provides a `Theme` on its own owner, then hosts a child component that
    /// reads it — the child (a descendant) must resolve the ancestor's value.
    struct Provider {
        sink: Arc<Mutex<Option<i32>>>,
    }
    impl Component for Provider {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            provide_context(Theme(7));
            any(component(Reader {
                sink: self.sink.clone(),
            }))
        }
    }

    /// Reads `Theme` from context during build, recording what it resolved.
    struct Reader {
        sink: Arc<Mutex<Option<i32>>>,
    }
    impl Component for Reader {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            *self.sink.lock().unwrap() = use_context::<Theme>().map(|t| t.0);
            any(Empty)
        }
    }

    /// Provides a `Theme` but does not descend into a reader — used to prove a
    /// sibling's provided context is invisible to a cousin.
    struct SecretProvider;
    impl Component for SecretProvider {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            provide_context(Theme(99));
            any(Empty)
        }
    }

    #[test]
    fn use_context_resolves_ancestor_provided_value() {
        let _owner = ambient();
        let sink = Arc::new(Mutex::new(None));
        let v = component(Provider { sink: sink.clone() });
        let _widget = build_widget::<(), _>(&v);
        assert_eq!(*sink.lock().unwrap(), Some(7));
    }

    #[test]
    fn sibling_does_not_see_a_cousins_context() {
        let parent = ambient();
        let reader_sink = Arc::new(Mutex::new(Some(-1)));
        // Two siblings under the same parent owner: one provides a secret, the
        // other reads. The reader (a cousin, not a descendant) must see None.
        parent.with(|| {
            let secret = component(SecretProvider);
            let _s = build_widget::<(), _>(&secret);
            let reader = component(Reader {
                sink: reader_sink.clone(),
            });
            let _r = build_widget::<(), _>(&reader);
        });
        assert_eq!(*reader_sink.lock().unwrap(), None);
    }

    // --- Criterion 3: on_cleanup / owner disposal ---------------------------

    struct Disposable {
        probe: Arc<AtomicU32>,
    }
    impl Component for Disposable {
        type State = ();
        fn init(&self) {
            let probe = self.probe.clone();
            // Registered once (init runs once) on this component's owner.
            on_cleanup(move || {
                probe.fetch_add(1, Ordering::SeqCst);
            });
        }
        fn build(&self, _state: &mut ()) -> impl View<()> {
            any(Empty)
        }
    }

    #[test]
    fn anyview_type_swap_disposes_component_owner_once() {
        let _owner = ambient();
        let probe = Arc::new(AtomicU32::new(0));
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);

        let prev = any::<(), _>(component(Disposable {
            probe: probe.clone(),
        }));
        let mut element: Box<dyn Widget> = prev.build(&mut ctx);
        assert_eq!(probe.load(Ordering::SeqCst), 0);

        // Swap the AnyView to a different concrete type: the component is torn
        // down and its `on_cleanup` runs exactly once.
        let next = any::<(), _>(OtherLeaf);
        next.rebuild(&prev, &mut element, &mut ctx);
        assert_eq!(probe.load(Ordering::SeqCst), 1);

        // Dropping the replaced widget (already disposed) must not fire again.
        drop(element);
        assert_eq!(probe.load(Ordering::SeqCst), 1);
    }

    // --- Erasure at the boundary --------------------------------------------

    /// A component whose `build` returns a concrete view (no `any(..)`): the
    /// host erases it, so the pod's double-boxed element wraps the leaf widget.
    struct ConcreteComp;
    impl Component for ConcreteComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            Empty
        }
    }

    /// The same leaf, returned already erased: `AnyView::new` is idempotent, so
    /// the host does not wrap the `AnyView` in a second box.
    struct ErasedComp;
    impl Component for ErasedComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            any(Empty)
        }
    }

    /// Whether the component's child element is the leaf widget itself (one
    /// erasure), not a box around another boxed element (two).
    fn child_is_empty_widget<C: Component>(widget: &mut ComponentWidget<C>) -> bool {
        let boxed = widget
            .child
            .widget_mut()
            .downcast_mut::<Box<dyn Widget>>()
            .expect("double-boxed AnyView element");
        let any: &dyn Any = &**boxed;
        any.is::<EmptyWidget>()
    }

    #[test]
    fn build_result_is_erased_exactly_once_at_the_boundary() {
        let _owner = ambient();

        let concrete = component(ConcreteComp);
        let mut widget = build_widget::<(), _>(&concrete);
        assert!(child_is_empty_widget(&mut widget), "concrete body, build");
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::rebuild(&concrete, &concrete, &mut widget, &mut ctx);
        assert!(child_is_empty_widget(&mut widget), "concrete body, rebuild");

        let erased = component(ErasedComp);
        let mut widget = build_widget::<(), _>(&erased);
        assert!(child_is_empty_widget(&mut widget), "any(..) body, build");
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::rebuild(&erased, &erased, &mut widget, &mut ctx);
        assert!(child_is_empty_widget(&mut widget), "any(..) body, rebuild");
    }

    // --- The swap arm's orphan mark, across the state boundary --------------

    /// A component whose child view *type* flips on a shared flag: `Empty`
    /// first, `OtherLeaf` after — the `ComponentView::rebuild` swap arm's
    /// fixture (`if editing { field } else { label }` inside a component's
    /// `build`).
    struct SwapComp {
        swapped: Rc<Cell<bool>>,
    }
    impl Component for SwapComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            if self.swapped.get() {
                any(OtherLeaf)
            } else {
                any(Empty)
            }
        }
    }

    #[test]
    fn component_swap_arm_marks_the_orphan_only_on_a_live_chain() {
        /// Swap the component's child type with the *outer* chain either live
        /// or already broken, reporting `(pod still focused, orphan marked)`.
        fn swap_child(outer_live: bool) -> (bool, bool) {
            let _ = crate::event::take_focus_orphaned();
            let _owner = ambient();
            let swapped = Rc::new(Cell::new(false));
            let v1 = component(SwapComp {
                swapped: swapped.clone(),
            });
            let mut widget = build_widget::<(), _>(&v1);
            // The component's own child pod holds the recorded focus path.
            widget.child.set_focused(true);

            swapped.set(true);
            let v2 = component(SwapComp {
                swapped: swapped.clone(),
            });
            let mut next_id = 0u64;
            let mut ctx = BuildCtx::new(&mut next_id);
            // The chain the surrounding tree hands this component.
            ctx.set_has_focus(outer_live);
            View::<()>::rebuild(&v2, &v1, &mut widget, &mut ctx);
            (
                widget.child.is_focused(),
                crate::event::take_focus_orphaned(),
            )
        }

        let (still_focused, marked) = swap_child(true);
        assert!(
            !still_focused,
            "a swapped-in widget must not inherit the old focus link"
        );
        assert!(
            marked,
            "a live focus path dying inside the component's swap releases the session"
        );

        let (still_focused, marked) = swap_child(false);
        assert!(!still_focused, "the flag is dropped either way");
        assert!(
            !marked,
            "the same swap under an already-blurred ancestor owns no session to release"
        );
    }

    #[test]
    fn component_content_only_rebuild_keeps_focus_and_marks_nothing() {
        // The negative guard: the component always re-runs `build`, so a
        // same-type child rebuild happens every frame a component is on screen
        // — marking there would drop the keyboard continuously.
        let _ = crate::event::take_focus_orphaned();
        let _owner = ambient();
        let v1 = component(Labeled { text: "a" });
        let mut widget = build_widget::<(), _>(&v1);
        widget.child.set_focused(true);

        let v2 = component(Labeled { text: "b" });
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::rebuild(&v2, &v1, &mut widget, &mut ctx);

        assert!(
            widget.child.is_focused(),
            "a content-only rebuild keeps the recorded focus path"
        );
        assert!(
            !crate::event::take_focus_orphaned(),
            "an in-place child rebuild must not orphan the focus session"
        );
    }

    #[test]
    fn explicit_teardown_disposes_owner_once_and_drop_is_idempotent() {
        let _owner = ambient();
        let probe = Arc::new(AtomicU32::new(0));
        let view = component(Disposable {
            probe: probe.clone(),
        });
        let mut widget = build_widget::<(), _>(&view);

        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::teardown(&view, &mut widget, &mut ctx);
        assert_eq!(probe.load(Ordering::SeqCst), 1);

        // The defensive `Drop` must not run the cleanup a second time.
        drop(widget);
        assert_eq!(probe.load(Ordering::SeqCst), 1);
    }

    // --- Criterion 4: event routing across the boundary ---------------------

    /// A widget that captures the pointer on `Down` (tracking `pressed`), and
    /// clears both on `Up`/`Cancel` — the latter without touching state.
    struct CaptureWidget {
        pressed: bool,
    }
    impl Widget for CaptureWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 40.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        self.pressed = true;
                        ctx.capture_pointer();
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Move => return EventResult::Handled,
                    // Cancel arm never reads application state.
                    PointerPhase::Up | PointerPhase::Cancel => {
                        self.pressed = false;
                        return EventResult::Handled;
                    }
                }
            }
            EventResult::Ignored
        }
    }
    struct CaptureView;
    impl View<()> for CaptureView {
        type Element = CaptureWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CaptureWidget {
            CaptureWidget { pressed: false }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut CaptureWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    struct CaptureComp;
    impl Component for CaptureComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            any(CaptureView)
        }
    }

    /// Recover the concrete inner widget from a component widget's pod (the
    /// child element is double-boxed).
    fn inner_widget<C: Component, W: Widget>(w: &mut ComponentWidget<C>) -> &mut W {
        w.child
            .widget_mut()
            .downcast_mut::<Box<dyn Widget>>()
            .expect("double-boxed AnyView element")
            .downcast_mut::<W>()
            .expect("inner concrete widget")
    }

    #[test]
    fn capture_survives_drag_outside_bounds_and_mirrors_outward() {
        let _owner = ambient();
        let view = component(CaptureComp);
        let mut widget = build_widget::<(), _>(&view);
        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::tight(Size::new(40.0, 40.0)));

        let mut outer = ();
        // Down inside: the inner widget captures and the flag mirrors outward.
        let captured = {
            let mut ectx = EventCtx::new(&mut outer, Point::ZERO, Size::new(40.0, 40.0));
            widget.event(&mut ectx, &pointer(PointerPhase::Down, 5.0, 5.0));
            ectx.is_pointer_captured()
        };
        assert!(captured, "inner capture bubbles to the outer context");
        assert!(
            widget.child.is_active(),
            "component pod records the capture"
        );

        // A Move far outside the child's bounds still reaches it (captured path).
        {
            let mut ectx = EventCtx::new(&mut outer, Point::ZERO, Size::new(40.0, 40.0));
            let r = widget.event(&mut ectx, &pointer(PointerPhase::Move, 500.0, 500.0));
            assert_eq!(r, EventResult::Handled, "drag outside bounds still routed");
        }
        assert!(widget.child.is_active(), "capture survives the drag");

        // Up releases the capture at the boundary.
        {
            let mut ectx = EventCtx::new(&mut outer, Point::ZERO, Size::new(40.0, 40.0));
            widget.event(&mut ectx, &pointer(PointerPhase::Up, 500.0, 500.0));
        }
        assert!(!widget.child.is_active(), "Up auto-releases the capture");
        assert!(!inner_widget::<_, CaptureWidget>(&mut widget).pressed);
    }

    #[test]
    fn synthesized_cancel_over_unit_state_is_safe_and_clears_pressed() {
        let _owner = ambient();
        let view = component(CaptureComp);
        let mut widget = build_widget::<(), _>(&view);
        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::tight(Size::new(40.0, 40.0)));

        // Arm the capture first (real state path).
        let mut outer = ();
        {
            let mut ectx = EventCtx::new(&mut outer, Point::ZERO, Size::new(40.0, 40.0));
            widget.event(&mut ectx, &pointer(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(inner_widget::<_, CaptureWidget>(&mut widget).pressed);

        // A structural-rebuild `Cancel` arrives over throwaway `()` state — as
        // `cancel_pod` synthesizes it — and must neither panic nor touch state.
        let mut dummy = ();
        {
            let mut ectx = EventCtx::new(&mut dummy, Point::ZERO, Size::new(40.0, 40.0));
            widget.event(&mut ectx, &pointer(PointerPhase::Cancel, 0.0, 0.0));
        }
        assert!(
            !inner_widget::<_, CaptureWidget>(&mut widget).pressed,
            "Cancel cleared pressed across the boundary"
        );
        assert!(!widget.child.is_active(), "Cancel released the capture");
    }

    // A leaf that asks for a cursor on every `Move` — the canonical request shape.
    struct CursorLeafWidget;
    impl Widget for CursorLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 40.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Move
            {
                ctx.set_cursor(CursorIcon::Text);
            }
            EventResult::Ignored
        }
    }
    struct CursorLeafView;
    impl View<()> for CursorLeafView {
        type Element = CursorLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CursorLeafWidget {
            CursorLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut CursorLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    struct CursorComp;
    impl Component for CursorComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            any(CursorLeafView)
        }
    }

    /// The component boundary is transparent to a cursor request with no mirroring
    /// code at all — the request rides one pass-scoped slot rather than a
    /// per-context field, so a refactor that turned it into a field (and forgot
    /// this carrier, as every other channel here has to be threaded) would drop it.
    #[test]
    fn a_cursor_request_crosses_the_component_boundary() {
        let _owner = ambient();
        let view = component(CursorComp);
        let mut widget = build_widget::<(), _>(&view);
        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::tight(Size::new(40.0, 40.0)));

        // `RenderRoot::event` clears the slot per pass; this test drives the widget
        // directly, so it does the same before asserting on what the pass left.
        clear_cursor_request();
        let mut outer = ();
        {
            let mut ectx = EventCtx::new(&mut outer, Point::ZERO, Size::new(40.0, 40.0));
            widget.event(&mut ectx, &pointer(PointerPhase::Move, 5.0, 5.0));
        }
        assert_eq!(
            take_cursor_request(),
            Some(CursorIcon::Text),
            "a request from below the component boundary reaches the root"
        );
    }

    // A focus + IME publishing leaf: focuses on a left-half Down and publishes
    // an IME surface; a right-half Down neither focuses nor publishes.
    struct ImeLeafWidget;
    impl Widget for ImeLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 100.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    if p.position.x < 50.0 {
                        ctx.request_focus();
                        ctx.publish_ime_state(ImeState {
                            active: true,
                            editing: EditingState {
                                text: "abc".to_string(),
                                selection_base: 3,
                                selection_extent: 3,
                                composing_base: -1,
                                composing_extent: -1,
                            },
                            caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
                            content_type: Default::default(),
                            suppress_soft_keyboard: false,
                        });
                    }
                    EventResult::Handled
                }
                InputEvent::Key(_) | InputEvent::Ime(_) => EventResult::Handled,
                _ => EventResult::Ignored,
            }
        }
    }
    struct ImeLeafView;
    impl View<()> for ImeLeafView {
        type Element = ImeLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeLeafWidget {
            ImeLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ImeLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    struct ImeComp;
    impl Component for ImeComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            any(ImeLeafView)
        }
    }

    #[test]
    fn focus_ime_and_blur_surface_through_render_root() {
        use crate::app::RenderRoot;
        let _owner = ambient();
        let mut root: RenderRoot<(), ComponentView<ImeComp>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut |_s: &mut ()| component(ImeComp), &mut state);
        root.layout(Size::new(100.0, 100.0));

        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        // A left-half Down inside the component focuses the inner leaf and its
        // IME surface bubbles across the boundary to the render root.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active(), "focus request crossed the boundary");
        let ime = root.ime_state().expect("IME surface surfaced at the root");
        assert!(ime.active);
        assert_eq!(ime.editing.text, "abc");

        // A focused Key routes down the focus path across the boundary (handled).
        let outcome = root.event(&mut state, &key_enter());
        assert!(outcome.handled, "Key routed to the focused inner leaf");

        // A right-half Down does not re-establish focus: blur clears both the
        // focus state and the IME surface at the root.
        root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 10.0));
        assert!(!root.is_focus_active(), "blur crossed the boundary");
        assert!(root.ime_state().is_none());
    }

    // --- Criterion 5: ChangeFlags propagation -------------------------------

    struct TextLeafView {
        text: &'static str,
    }
    struct TextLeafWidget {
        text: &'static str,
    }
    impl View<()> for TextLeafView {
        type Element = TextLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TextLeafWidget {
            TextLeafWidget { text: self.text }
        }
        fn rebuild(
            &self,
            prev: &Self,
            element: &mut TextLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.text != self.text {
                element.text = self.text;
                ChangeFlags::PAINT
            } else {
                ChangeFlags::NONE
            }
        }
    }
    impl Widget for TextLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }
    struct Labeled {
        text: &'static str,
    }
    impl Component for Labeled {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            any(TextLeafView { text: self.text })
        }
    }

    #[test]
    fn child_paint_flags_bubble_out_of_component_rebuild() {
        let _owner = ambient();
        let v1 = component(Labeled { text: "a" });
        let mut widget = build_widget::<(), _>(&v1);

        // An unchanged rebuild reports nothing.
        let v_same = component(Labeled { text: "a" });
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        let flags = View::<()>::rebuild(&v_same, &v1, &mut widget, &mut ctx);
        assert_eq!(flags, ChangeFlags::NONE);

        // A changed prop makes the child view return PAINT; it must bubble out
        // of the component rebuild unchanged.
        let v2 = component(Labeled { text: "b" });
        let flags = View::<()>::rebuild(&v2, &v_same, &mut widget, &mut ctx);
        assert_eq!(flags, ChangeFlags::PAINT);
    }

    // --- Hot-patch seam: the creator-image layout witness --------------------

    #[cfg(feature = "hotpatch")]
    mod seam {
        use super::*;
        use crate::hotpatch::{Inert, SeamWitness};
        use std::any::type_name;
        use std::mem::offset_of;

        /// A witness no image computes for `C`: the stored layout of a widget
        /// another image built.
        fn foreign<C: Component>() -> SeamWitness {
            let own = SeamWitness::of::<C>();
            SeamWitness {
                size_state: own.size_state + 8,
                ..own
            }
        }

        /// Whether a mismatch for `S` is pending; clears this test's records.
        fn take_reported<S>() -> bool {
            let mine: Vec<_> = frust_hotpatch::pending_layout_mismatches()
                .into_iter()
                .filter(|m| m.type_name == type_name::<S>())
                .collect();
            frust_hotpatch::mark_layout_mismatches_reported(&mine);
            !mine.is_empty()
        }

        /// A state whose drop is observable. Each test names its own type so
        /// the process-global mismatch records never cross tests.
        struct Counted<const N: usize> {
            value: u32,
            drops: Arc<AtomicU32>,
        }
        impl<const N: usize> Drop for Counted<N> {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        /// A component over `Counted<N>` counting its builds, its owner's
        /// disposal and its child's teardowns.
        struct Probe<const N: usize> {
            builds: Arc<AtomicU32>,
            drops: Arc<AtomicU32>,
            disposed: Arc<AtomicU32>,
            child_teardowns: Arc<AtomicU32>,
        }
        impl<const N: usize> Probe<N> {
            fn new() -> Self {
                Self {
                    builds: Arc::new(AtomicU32::new(0)),
                    drops: Arc::new(AtomicU32::new(0)),
                    disposed: Arc::new(AtomicU32::new(0)),
                    child_teardowns: Arc::new(AtomicU32::new(0)),
                }
            }
            fn handle(&self) -> Self {
                Self {
                    builds: self.builds.clone(),
                    drops: self.drops.clone(),
                    disposed: self.disposed.clone(),
                    child_teardowns: self.child_teardowns.clone(),
                }
            }
        }
        impl<const N: usize> Component for Probe<N> {
            type State = Counted<N>;
            fn init(&self) -> Counted<N> {
                let disposed = self.disposed.clone();
                on_cleanup(move || {
                    disposed.fetch_add(1, Ordering::SeqCst);
                });
                Counted {
                    value: 0,
                    drops: self.drops.clone(),
                }
            }
            fn build(&self, state: &mut Counted<N>) -> impl View<Counted<N>> {
                self.builds.fetch_add(1, Ordering::SeqCst);
                state.value += 1;
                TeardownProbe {
                    teardowns: self.child_teardowns.clone(),
                }
            }
        }

        /// A leaf counting its teardowns.
        struct TeardownProbe {
            teardowns: Arc<AtomicU32>,
        }
        impl<S: 'static> View<S> for TeardownProbe {
            type Element = EmptyWidget;
            fn build(&self, _ctx: &mut BuildCtx<'_>) -> EmptyWidget {
                EmptyWidget
            }
            fn rebuild(
                &self,
                _prev: &Self,
                _element: &mut EmptyWidget,
                _ctx: &mut BuildCtx<'_>,
            ) -> ChangeFlags {
                ChangeFlags::NONE
            }
            fn teardown(&self, _element: &mut EmptyWidget, _ctx: &mut BuildCtx<'_>) {
                self.teardowns.fetch_add(1, Ordering::SeqCst);
            }
        }

        /// Hosts `Probe<1>` as its subtree, so the nested widget sits in its
        /// own erased box.
        struct Parent {
            nested: Probe<1>,
        }
        impl Component for Parent {
            type State = ();
            fn init(&self) {}
            fn build(&self, _state: &mut ()) -> impl View<()> {
                component(self.nested.handle())
            }
        }

        fn nested_of(parent: &mut ComponentWidget<Parent>) -> &mut ComponentWidget<Probe<1>> {
            let boxed = parent
                .child
                .widget_mut()
                .downcast_mut::<Box<dyn Widget>>()
                .expect("component child element is a boxed AnyView widget");
            (**boxed)
                .downcast_mut::<ComponentWidget<Probe<1>>>()
                .expect("the parent's subtree is the nested component")
        }

        #[test]
        fn a_nested_widget_with_a_foreign_witness_is_never_rebuilt() {
            let _owner = ambient();
            let nested = Probe::<1>::new();
            let v1 = component(Parent {
                nested: nested.handle(),
            });
            let mut widget = build_widget::<(), _>(&v1);
            assert_eq!(nested.builds.load(Ordering::SeqCst), 1);

            nested_of(&mut widget).set_witness_for_test(foreign::<Probe<1>>());
            let v2 = component(Parent {
                nested: nested.handle(),
            });
            let mut next_id = 0u64;
            let mut ctx = BuildCtx::new(&mut next_id);
            let flags = View::<()>::rebuild(&v2, &v1, &mut widget, &mut ctx);

            assert_eq!(flags, ChangeFlags::NONE);
            assert_eq!(nested.builds.load(Ordering::SeqCst), 1, "never rebuilt");
            assert_eq!(nested_of(&mut widget).state().value, 1, "state untouched");
            assert!(take_reported::<Counted<1>>(), "the mismatch is reported");

            // Dropping the tree leaks the mismatched state and disposes its owner.
            drop(widget);
            assert_eq!(nested.drops.load(Ordering::SeqCst), 0);
            assert_eq!(nested.disposed.load(Ordering::SeqCst), 1);
            assert!(take_reported::<Counted<1>>());
        }

        /// A two-arm view whose element holds the live arm inline, the shape of
        /// `frust-widgets`' `Either` (which this crate cannot depend on).
        enum TwoArm<L, R> {
            Left(L),
            Right(R),
        }
        enum TwoArmWidget<L, R> {
            Left(L),
            Right(R),
        }
        impl<L: Widget, R: Widget> Widget for TwoArmWidget<L, R> {
            fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                match self {
                    Self::Left(w) => w.layout(ctx, bc),
                    Self::Right(w) => w.layout(ctx, bc),
                }
            }
            fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                match self {
                    Self::Left(w) => w.paint(ctx, scene),
                    Self::Right(w) => w.paint(ctx, scene),
                }
            }
        }
        impl<S: 'static, L: View<S>, R: View<S>> View<S> for TwoArm<L, R> {
            type Element = TwoArmWidget<L::Element, R::Element>;
            fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
                match self {
                    Self::Left(v) => TwoArmWidget::Left(v.build(ctx)),
                    Self::Right(v) => TwoArmWidget::Right(v.build(ctx)),
                }
            }
            fn rebuild(
                &self,
                prev: &Self,
                element: &mut Self::Element,
                ctx: &mut BuildCtx<'_>,
            ) -> ChangeFlags {
                match (self, prev, &mut *element) {
                    (Self::Left(v), Self::Left(p), TwoArmWidget::Left(w)) => v.rebuild(p, w, ctx),
                    (Self::Right(v), Self::Right(p), TwoArmWidget::Right(w)) => {
                        v.rebuild(p, w, ctx)
                    }
                    _ => {
                        // Arm swap: tear the old arm down, then build the new
                        // one in place, dropping the old element inline.
                        prev.teardown(element, ctx);
                        *element = self.build(ctx);
                        ChangeFlags::LAYOUT | ChangeFlags::PAINT
                    }
                }
            }
            fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
                match (self, element) {
                    (Self::Left(v), TwoArmWidget::Left(w)) => v.teardown(w, ctx),
                    (Self::Right(v), TwoArmWidget::Right(w)) => v.teardown(w, ctx),
                    _ => {}
                }
            }
        }

        #[test]
        fn an_inline_arm_swap_leaks_a_foreign_state_and_disposes_its_owner() {
            let _owner = ambient();
            let probe = Probe::<2>::new();
            let left: TwoArm<ComponentView<Probe<2>>, Empty> =
                TwoArm::Left(component(probe.handle()));
            let mut next_id = 0u64;
            let mut ctx = BuildCtx::new(&mut next_id);
            let mut element = View::<()>::build(&left, &mut ctx);
            match &mut element {
                TwoArmWidget::Left(w) => w.set_witness_for_test(foreign::<Probe<2>>()),
                TwoArmWidget::Right(_) => unreachable!("built the left arm"),
            }

            let right: TwoArm<ComponentView<Probe<2>>, Empty> = TwoArm::Right(Empty);
            let flags = View::<()>::rebuild(&right, &left, &mut element, &mut ctx);

            assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
            assert!(matches!(element, TwoArmWidget::Right(_)), "swapped out");
            assert_eq!(
                probe.drops.load(Ordering::SeqCst),
                0,
                "the state is neither dropped nor freed"
            );
            assert_eq!(
                probe.child_teardowns.load(Ordering::SeqCst),
                0,
                "teardown did not proceed"
            );
            assert_eq!(probe.disposed.load(Ordering::SeqCst), 1, "owner disposed");
            assert!(take_reported::<Counted<2>>(), "the mismatch is reported");
        }

        #[test]
        fn a_matching_widget_tears_down_and_drops_its_state() {
            let _owner = ambient();
            let probe = Probe::<3>::new();
            let view = component(probe.handle());
            let mut widget = build_widget::<(), _>(&view);
            let mut next_id = 0u64;
            let mut ctx = BuildCtx::new(&mut next_id);
            View::<()>::rebuild(&view, &view, &mut widget, &mut ctx);
            assert_eq!(probe.builds.load(Ordering::SeqCst), 2);
            View::<()>::teardown(&view, &mut widget, &mut ctx);
            assert_eq!(probe.child_teardowns.load(Ordering::SeqCst), 1);
            assert_eq!(probe.disposed.load(Ordering::SeqCst), 1);
            drop(widget);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
            assert!(!take_reported::<Counted<3>>());
        }

        struct Tiny;
        impl Component for Tiny {
            type State = u8;
            fn init(&self) -> u8 {
                0
            }
            fn build(&self, _state: &mut u8) -> impl View<u8> {
                Inert
            }
        }

        struct Huge;
        impl Component for Huge {
            type State = [u64; 32];
            fn init(&self) -> [u64; 32] {
                [0; 32]
            }
            fn build(&self, _state: &mut [u64; 32]) -> impl View<[u64; 32]> {
                Inert
            }
        }

        #[test]
        fn widget_layout_does_not_depend_on_the_state_layout() {
            assert_ne!(size_of::<u8>(), size_of::<[u64; 32]>());
            assert_eq!(
                size_of::<ComponentWidget<Tiny>>(),
                size_of::<ComponentWidget<Huge>>()
            );
            assert_eq!(
                align_of::<ComponentWidget<Tiny>>(),
                align_of::<ComponentWidget<Huge>>()
            );
            macro_rules! same_offset {
                ($($field:ident),*) => {$(
                    assert_eq!(
                        offset_of!(ComponentWidget<Tiny>, $field),
                        offset_of!(ComponentWidget<Huge>, $field),
                        stringify!($field)
                    );
                )*};
            }
            same_offset!(witness, state, prev, child, owner, next_id, disposed);
            assert_eq!(offset_of!(ComponentWidget<Tiny>, witness), 0);
        }
    }
}
