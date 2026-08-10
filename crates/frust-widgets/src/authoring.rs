//! The widget-authoring toolkit: the shared child plumbing, event routing and
//! callback erasure every container/interactive widget is assembled from — the
//! surface for authoring widgets, and whole design systems, **outside this
//! crate**.
//!
//! A container (a `View`/`Widget` pair owning children) or an interactive leaf
//! (one reporting a press back into application state) needs three things
//! `frust-core` deliberately does not provide:
//!
//! - **Child plumbing**: [`build_child`]/[`rebuild_child`]/[`teardown_child`] for
//!   a single [`AnyView`] child, and [`rebuild_children`] for a `Vec` of them
//!   (positional or [`ChildKey`]-keyed reconciliation, with the focus/capture
//!   retention rules a hand-rolled diff gets wrong).
//! - **Event routing**: [`route_event`] (multi-child containers, reverse paint
//!   order + capture/focus fast paths) and [`route_event_single`] (one-child
//!   wrappers).
//! - **Callback erasure**: [`erase_callback`]/[`erase_callback_arg`], turning a
//!   view-held `Rc<dyn Fn(&mut State)>` into the [`ErasedCallback`]/
//!   [`ErasedArgCallback`] adapter a non-generic widget holds, so the retained
//!   widget never becomes generic over the application-state type.
//! - **Child visitation**: [`visit_children!`] over [`VisitPods`], writing a
//!   container's [`Widget::visit_children`] body from its child fields — the
//!   read-only seam that lets tooling walk into a container's retained subtree
//!   (`frust-core`'s `WidgetTree::inspect`). One line per container; a leaf
//!   needs nothing.
//! - **Themed text roles**: [`ThemeTextColor`] (re-exported here) and
//!   [`TextView::themed_role`](crate::TextView::themed_role) — how a widget labels
//!   a child [`text`](crate::text) run with the themed color role it should
//!   default to, instead of hardcoding a color.
//!
//! **Application code should prefer `frust::authoring`**, which re-exports
//! everything below plus the `frust-core` trait vocabulary and `kurbo`/`peniko`
//! geometry — so an app depends on `frust` alone. The example spells its imports
//! the long way only because this crate cannot name `frust` without a
//! dev-dependency cycle; `frust::authoring`'s own module docs carry the
//! facade-spelled version.
//!
//! **Stability: pre-1.0, and a real supported public API** — not `#[doc(hidden)]`
//! plumbing, so a change to any signature here is a **breaking change**, released
//! as such. The three catalogs shipping with this crate (`material`, `cupertino`,
//! `glyph`) consume exactly this surface and nothing more, so a third-party design
//! system authored against it is at parity with the built-ins by construction.
//!
//! # Example: a one-child container widget
//!
//! A container that offsets its single child, wired through the full lifecycle —
//! build, rebuild, teardown, layout, paint, event routing, semantics forwarding:
//!
//! ```
//! use frust_core::{
//!     AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
//!     InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
//! };
//! use frust_widgets::authoring::{build_child, rebuild_child, route_event_single, teardown_child};
//! use kurbo::{Point, Size};
//!
//! /// The declarative half: a child plus the offset to apply to it.
//! struct OffsetView<State: 'static> {
//!     offset: Point,
//!     child: AnyView<State>,
//! }
//!
//! /// The view-fn app code calls; `any` erases the concrete child view.
//! fn offset<State: 'static, V: View<State>>(offset: Point, child: V) -> OffsetView<State> {
//!     OffsetView { offset, child: any(child) }
//! }
//!
//! /// The retained half: the live child pod plus the applied offset.
//! struct OffsetWidget {
//!     offset: Point,
//!     child: ChildPod,
//! }
//!
//! impl<State: 'static> View<State> for OffsetView<State> {
//!     type Element = OffsetWidget;
//!
//!     fn build(&self, ctx: &mut BuildCtx<'_>) -> OffsetWidget {
//!         OffsetWidget { offset: self.offset, child: build_child(&self.child, ctx) }
//!     }
//!
//!     fn rebuild(
//!         &self,
//!         prev: &Self,
//!         element: &mut OffsetWidget,
//!         ctx: &mut BuildCtx<'_>,
//!     ) -> ChangeFlags {
//!         let mut flags = ChangeFlags::NONE;
//!         if prev.offset != self.offset {
//!             element.offset = self.offset;
//!             flags |= ChangeFlags::LAYOUT;
//!         }
//!         flags | rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
//!     }
//!
//!     fn teardown(&self, element: &mut OffsetWidget, ctx: &mut BuildCtx<'_>) {
//!         teardown_child(&self.child, &mut element.child, ctx);
//!     }
//! }
//!
//! impl Widget for OffsetWidget {
//!     fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
//!         let size = self.child.layout_child(ctx, bc);
//!         self.child.set_origin(self.offset);
//!         bc.constrain(size)
//!     }
//!
//!     fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
//!         self.child.paint_child(ctx, scene);
//!     }
//!
//!     fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
//!         // Never re-hit-test a captured child by hand — this helper owns the
//!         // capture/focus fast paths and the blur-on-outside-tap rule.
//!         route_event_single(&mut self.child, ctx, event)
//!     }
//!
//!     fn semantics(&self, ctx: &mut SemanticsCtx) {
//!         // A transparent wrapper still MUST forward, or the child's whole
//!         // subtree drops out of the accessibility tree.
//!         self.child.semantics_child(ctx);
//!     }
//!
//!     // The read-only tooling seam: publish every pod this widget owns.
//!     frust_widgets::authoring::visit_children!(child);
//! }
//! # fn main() {}
//! ```

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frust_core::{
    AnyView, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, PointerButton, PointerEvent,
    PointerPhase, View, Widget,
};
use kurbo::Point;

use crate::ChildKey;

pub use crate::image::ImageSource;
pub use crate::text::ThemeTextColor;

/// Pressed-state overlay opacity (source: androidx Compose Material3
/// `StateTokens` v0_210, retrieved 2026-07-17 — supersedes material-web
/// v0.192's 12%).
///
/// Lives here, not with the rest of the M3 state-layer table, because it is the
/// one interaction opacity a *language-neutral* widget needs: a press overlay is
/// a mechanism (how a control acknowledges a touch), not a Material design
/// decision. A design system's own state-layer table is free to differ; this is
/// the value the baseline widgets and the built-in catalogs share.
pub const PRESSED_OPACITY: f32 = 0.10;

/// A widget-held, `State`-erased app-state callback adapter (see
/// [`erase_callback`]).
pub type ErasedCallback = Box<dyn FnMut(&mut EventCtx)>;

/// A widget-held, `State`-erased callback adapter carrying one value argument
/// (see [`erase_callback_arg`]).
pub type ErasedArgCallback<A> = Box<dyn FnMut(&mut EventCtx, A)>;

/// A view-held, typed callback carrying one value argument (Checkbox's `bool`,
/// Slider's `f64`), erased to [`ErasedArgCallback`] on build.
pub type TypedArgCallback<State, A> = std::rc::Rc<dyn Fn(&mut State, A)>;

/// Erase a view-held `Rc<dyn Fn(&mut State)>` app-state callback into the
/// widget-held [`ErasedCallback`] adapter the interactive widgets invoke during
/// the event pass.
///
/// The closure recovers the concrete `State` from the type-erased [`EventCtx`]
/// with [`EventCtx::state_mut`] (the downcast happens *inside* the adapter), so
/// the widget itself stays non-generic over `State`. Closures aren't comparable,
/// so `build`/`rebuild` reinstall the adapter unconditionally — it's cheap.
pub fn erase_callback<State: 'static>(callback: &Rc<dyn Fn(&mut State)>) -> ErasedCallback {
    let callback = callback.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state);
    })
}

/// Like [`erase_callback`], but for callbacks that also carry a value argument
/// (Checkbox's `bool`, Slider's `f64`).
pub fn erase_callback_arg<State: 'static, A: 'static>(
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
///
/// The build runs with the focus chain closed
/// ([`BuildCtx::with_focus_link`]`(false, …)`): the pod being built is brand new,
/// so by construction it holds no focus link — the same composition
/// [`rebuild_child`] performs, kept uniform so "descending into a pod always
/// recomputes the chain" has no exceptions.
pub fn build_child<State: 'static>(view: &AnyView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    let element: Box<dyn Widget> = ctx.with_focus_link(false, |ctx| view.build(ctx));
    ChildPod::new(Box::new(element))
}

/// Reconcile one [`AnyView`] child in place through its `ChildPod`.
///
/// A type-swapped child (see `rebuild_child_tracked`) that still holds a
/// recorded capture has that capture dropped: `pod.set_active(false)`, no
/// synthetic `Cancel`. The old armed widget was torn down inside
/// `AnyView::rebuild` — its state died with it — and the fresh widget in its
/// place never saw the original `Down`, so there is nothing to unwind; this
/// mirrors [`rebuild_children`]'s documented type-swap semantics for the
/// single-child wrappers (`Padding`/`Align`/`SizedBox`, and the interactive
/// widgets' own label/track children) that call this instead of
/// `rebuild_children`.
///
/// **A type swap severs the recorded focus path exactly like it severs the
/// capture path**, and is handled the same way the keyed reconciler handles its
/// own swap arm: the pod's `focused` flag is dropped (the fresh widget never
/// claimed focus — leaving the link set would route `Key`/`Ime` events into a
/// widget that ignores them) and
/// [`mark_focus_orphaned`](frust_core::mark_focus_orphaned) is raised *when the
/// severed link was on the live focus chain* ([`mark_orphan_if_live`]), so
/// `RenderRoot::rebuild` releases `focus_active`/`ime_state` before the frame
/// ends. This is the `Padding(if editing { text_input } else { text })` shape:
/// without the release the keyboard stays up over an idle screen and
/// `is_focus_active()` keeps reporting a session whose owner no longer exists. A
/// swap of a merely *stale* flag under an already-blurred ancestor marks
/// nothing — it owned no session.
pub fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    // Read before the nested rebuild, exactly as `rebuild_child_tracked` reads
    // its own `link_focused`: the gate describes the link this pod held *going
    // into* the swap.
    let link_focused = pod.is_focused();
    let (flags, swapped) = rebuild_child_tracked(prev, next, pod, ctx);
    if swapped {
        if pod.is_active() {
            pod.set_active(false);
        }
        if link_focused {
            pod.set_focused(false);
        }
        // `ctx` carries the chain down to *this wrapper*; the pod's own flag is
        // ANDed onto it inside, the same composition every other marking site
        // uses.
        mark_orphan_if_live(ctx, link_focused);
    }
    flags
}

/// Like [`rebuild_child`], but also reports whether the rebuild *replaced* the
/// underlying widget (an [`AnyView`] concrete-type swap) rather than mutating it
/// in place.
///
/// The swap flag drives both callers' capture bookkeeping: [`rebuild_children`]'s
/// (multi-child `Vec` containers — `Flex`/`Stack`) and [`rebuild_child`]'s
/// (single-child wrappers). A fresh widget swapped in at a still-captured
/// index/pod never saw the original `Down`, so its stale `active` path is
/// dropped without a synthetic `Cancel` (there is nothing armed to unwind).
/// Detection compares the boxed element's concrete
/// [`TypeId`](std::any::TypeId) across the rebuild.
///
/// **This is the descent that extends the focus chain.** The nested rebuild runs
/// under [`BuildCtx::with_focus_link`]`(pod.is_focused(), …)`, so a container
/// deeper in the tree sees `ctx.has_focus()` == "is the whole chain from the root
/// to me focused" — the value the orphan-marking sites gate on. Every route into
/// a child's `rebuild`/`teardown` funnels through here (or through
/// [`teardown_child`]), which is what makes the chain complete through arbitrary
/// nesting.
fn rebuild_child_tracked<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> (ChangeFlags, bool) {
    // Read before the `widget_mut` borrow below.
    let link_focused = pod.is_focused();
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("layout-container child element is a boxed AnyView widget");
    let before = {
        let any: &dyn Any = &**element;
        any.type_id()
    };
    let flags = ctx.with_focus_link(link_focused, |ctx| next.rebuild(prev, element, ctx));
    let after = {
        let any: &dyn Any = &**element;
        any.type_id()
    };
    (flags, before != after)
}

/// Tear down one [`AnyView`] child through its `ChildPod`.
///
/// A pod still holding an in-flight capture ([`ChildPod::is_active`]) is
/// cancelled (a synthetic `Cancel`, see `cancel_pod`) before teardown, so an
/// armed widget dropped mid-gesture (e.g. the active row truncated out of a
/// shrinking list) unwinds its state machine instead of vanishing with no
/// terminating `Up`/`Cancel`.
///
/// A pod still holding the recorded **focus** path ([`ChildPod::is_focused`])
/// *on the live focus chain* ([`BuildCtx::has_focus`]) raises
/// [`mark_focus_orphaned`](frust_core::mark_focus_orphaned) instead: the focused
/// widget is about to stop existing, so the whole focus/IME session dies with it
/// and `RenderRoot::rebuild` releases it before the frame ends (see
/// [`cancel_active_children`]'s note for the full mechanism, and why the pod
/// cannot do it itself). A pod whose flag is stale — set, but under a link some
/// ancestor already cleared — is torn down silently: it owns no session to lose.
pub fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if pod.is_active() {
        cancel_pod(pod);
        pod.set_active(false);
    }
    let link_focused = pod.is_focused();
    if link_focused {
        // The pod is dropped by the caller right after this returns, so clearing
        // the flag is bookkeeping hygiene, not the load-bearing part — the mark is.
        pod.set_focused(false);
    }
    mark_orphan_if_live(ctx, link_focused);
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        // Descend with the chain extended by the link this pod held *before* the
        // clear above: a focused pod's subtree is still on the live chain while it
        // is being torn down, so a focused descendant reports its own orphan.
        ctx.with_focus_link(link_focused, |ctx| view.teardown(element, ctx));
    }
}

/// Raise the orphan mark for a severed focus link, but **only when the link was
/// live** — the one gate every marking site in this module shares
/// ([`teardown_child`], [`cancel_active_children`], the keyed reconciler's
/// type-swap arm, and [`rebuild_child`]'s).
///
/// `link_focused` is the dying pod's own [`ChildPod::is_focused`] flag and
/// `ctx.has_focus()` is the chain above it, so the mark means exactly one thing:
/// **a LIVE session just lost its owner.** Both halves are required. The flag
/// alone is not evidence of a live session — a container-routed blur clears the
/// focus link at the nearest common ancestor only, leaving flags deeper in the
/// blurred branch legitimately set (see [`route_event`]) — and marking on the
/// flag alone is what let a recycled list row, a filtered-out item, or a switched
/// pattern release the *currently typed-into* field somewhere else in the tree.
/// The chain alone is not evidence either: a live chain running past an unfocused
/// sibling says nothing about that sibling.
fn mark_orphan_if_live(ctx: &BuildCtx<'_>, link_focused: bool) {
    if link_focused && ctx.has_focus() {
        frust_core::mark_focus_orphaned();
    }
}

/// Deliver a synthetic [`PointerPhase::Cancel`] to a captured child whose
/// in-flight gesture a structural rebuild has invalidated, so its state machine
/// unwinds instead of firing on a later `Up`.
///
/// # Cancel-during-rebuild contract
///
/// The rebuild pass runs over a [`BuildCtx`], not an [`EventCtx`] — there is no
/// application state in scope. This is sound *only because a `Cancel` handler
/// must never read application state* (`EventCtx::state_mut`): every
/// interactive widget's `Cancel` arm only clears internal flags. That invariant
/// lets this build a minimal [`EventCtx`] over a throwaway `()` state to drive
/// the unwind; a `Cancel` handler that reached for real state would panic here
/// on the `()` downcast — a deliberate tripwire, not a silent corruption.
pub(crate) fn cancel_pod(pod: &mut ChildPod) {
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

/// Cancel-and-clear the recorded interaction paths of the pods handed to it —
/// both the capture (`active`) path and the focus (`focused`) path — after a
/// structural change.
///
/// Called by [`rebuild_children_positional`] on the **tail past the stable
/// prefix** (indices `≥ k`, whose widget identity may have changed): a swapped
/// slot, a shifted pod, or the grown tail. The stable prefix (indices `< k`)
/// keeps the same live widget across the rebuild, so its recorded paths stay
/// valid and are *not* passed here — that is Flutter's focus/IME retention
/// invariant (see [`rebuild_children_positional`]). The keyed reconciler does not
/// call this at all: a key-matched pod relocates with its flags intact, and only
/// a torn-down or type-swapped pod (identity broken) is cleared inline there.
///
/// For each pod handed in, any surviving armed widget is unwound via [`cancel_pod`]
/// and its `active` flag dropped, and any focused child has its `focused` flag
/// dropped. `ctx` carries the focus chain down to the *container*
/// ([`BuildCtx::has_focus`]), which each pod's own flag is ANDed onto to decide
/// whether the clear severed a live session (see [`mark_orphan_if_live`]).
///
/// # Focus vs. capture: why one synthesizes a `Cancel` and the other does not
///
/// Capture state lives *inside* the widget (an `armed`/`pressed` flag its own
/// event arms), so dropping the recorded path requires a synthetic [`Cancel`]
/// ([`cancel_pod`]) to unwind that internal machine — otherwise it would fire on
/// a later hit-tested `Up`. Focus state, by contrast, is *reflected* from the pod
/// flag into the widget each event (`EventCtx::has_focus`, threaded through
/// [`ChildPod::event_child`]) rather than latched internally, so clearing the pod
/// flag is enough — there is no widget-internal blur to drive, and a `Cancel`
/// handler must not touch app state anyway (`docs/CODE_STANDARDS.md`).
///
/// # RenderRoot notification: the orphan mark
///
/// Clearing `focused` here cannot notify [`RenderRoot`](frust_core::RenderRoot)
/// directly — the rebuild pass runs over a [`BuildCtx`], with no `RenderRoot` in
/// scope, exactly as the capture-cancel path above cannot reset
/// `RenderRoot::pointer_captured`. It instead raises the thread-local
/// [`mark_focus_orphaned`](frust_core::mark_focus_orphaned) flag, which
/// `RenderRoot::rebuild` drains at the end of the same rebuild and turns into a
/// full focus/IME session release (`focus_active` cleared, the shell-facing
/// surface dropped, one edge on the focus/IME generation).
///
/// **Why the mark and not convergence.** Leaving the root's
/// `focus_active`/`ime_state` stale for the next event pass to self-correct is
/// fine while the user keeps touching the screen and wrong the moment they
/// stop: on an idle screen no next event arrives, so the root
/// keeps reporting a live focus session for a widget that no longer exists —
/// which strands `ime_state()` at the shell and (measured on a Xiaomi 12) held
/// the mobile frame gate's focus input up permanently after a navigator pop.
/// A session must die with its owner, not with the next tap.
///
/// **Why a thread-local and not a tree walk.** Validating the root's mirror
/// against reality after the diff would need to ask "does any pod in the tree
/// still hold focus?", and there is no such iterator: `Widget` exposes no
/// children, so nothing can walk the retained tree generically. The mark is the
/// only channel available from inside a `BuildCtx` pass, and it mirrors
/// `mark_pending_result_flush`'s shape exactly (data-free, idempotent,
/// UI-thread-affine).
///
/// **A stale flag deep in a blurred branch is harmless by construction.** A
/// container-routed blur clears the focus link at the nearest common ancestor
/// only, so `focused` flags *below* that link stay set until focus next enters
/// the subtree (see [`route_event`]). Such a pod is not evidence of a session:
/// the mark is gated on the whole chain ([`mark_orphan_if_live`]), which a
/// cleared ancestor link forces to `false` for the entire subtree below it —
/// exactly as the same composition already hides those flags from paint
/// (`ChildPod::paint_child`'s `self.focused && ctx.has_focus()`) and as
/// focus-path routing already refuses to deliver into them. Tearing down a stale
/// branch — a recycled list row, a filtered-out item, a switched pattern —
/// therefore leaves the session of whatever field is *actually* focused
/// untouched, instead of dropping the keyboard mid-typing somewhere unrelated.
fn cancel_active_children(pods: &mut [ChildPod], ctx: &BuildCtx<'_>) {
    for pod in pods.iter_mut() {
        if pod.is_active() {
            cancel_pod(pod);
            pod.set_active(false);
        }
        let link_focused = pod.is_focused();
        if link_focused {
            pod.set_focused(false);
        }
        mark_orphan_if_live(ctx, link_focused);
    }
}

/// Diff a child list against its live `ChildPod`s, extracting each child's
/// [`AnyView`] through `view_of` (identity for a plain `Vec<AnyView>`, `|c|
/// &c.view` for Flex's `FlexChild` wrapper) and its optional
/// [`ChildKey`] through `key_of` (`|_| None` for keyless containers, `|c| c.key`
/// for Flex) — the one shared reconciler for every multi-child container.
///
/// **Positional vs. keyed.** With *no* child carrying a key this is a positional
/// diff (see `rebuild_children_positional`); with keys present it matches
/// old↔new by key so reorders/inserts preserve widget identity and state (see
/// `rebuild_children_keyed`). Keys are all-or-nothing per list: a list that
/// mixes keyed and unkeyed children, or repeats a key, trips a `debug_assert`
/// and falls back to the positional path (correct, just identity-blind).
///
/// **A structural change preserves focus/capture for unchanged siblings
/// (Flutter's invariant).** A structural edit among siblings only cancels the
/// recorded capture/focus paths of children whose own identity actually changed:
/// the positional path preserves its stable prefix and cancels only the tail past
/// the first type swap; the keyed path relocates a key-matched child's paths
/// intact and cancels only a torn-down or type-swapped child. A
/// structural-change-free rebuild leaves every recorded path untouched, so an
/// ordinary every-frame rebuild never breaks a captured drag or dismisses the
/// keyboard for an unchanged child.
///
/// **A child that loses a LIVE focus path takes the root's session with it.**
/// Every route that severs a recorded focus path here — a torn-down child
/// ([`teardown_child`]), a cleared tail ([`cancel_active_children`]), a
/// key-reused type swap — raises
/// [`mark_focus_orphaned`](frust_core::mark_focus_orphaned) *when the severed
/// link was on the live focus chain* ([`mark_orphan_if_live`]), and
/// `RenderRoot::rebuild` releases `focus_active`/`ime_state` before the frame
/// ends rather than leaving them standing over a widget that no longer exists. A
/// severed link that was merely a stale flag deep in an already-blurred branch
/// marks nothing: it owned no session. See [`cancel_active_children`] for the
/// mechanism.
pub fn rebuild_children<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
    key_of: impl Fn(&C) -> Option<ChildKey>,
) -> ChangeFlags {
    let any_keyed = prev.iter().chain(next.iter()).any(|c| key_of(c).is_some());
    if !any_keyed {
        return rebuild_children_positional(prev, next, pods, ctx, view_of);
    }
    // v1 keys are all-or-nothing per list: a mixed list has no well-defined
    // match for its unkeyed members, so fall back to positional (identity-blind
    // but correct) rather than guess. A debug build flags the misuse loudly.
    let all_keyed = prev.iter().chain(next.iter()).all(|c| key_of(c).is_some());
    if !all_keyed {
        debug_assert!(
            false,
            "keyed child list mixes keyed and unkeyed children; \
             falling back to positional reconciliation"
        );
        return rebuild_children_positional(prev, next, pods, ctx, view_of);
    }
    rebuild_children_keyed(prev, next, pods, ctx, view_of, key_of)
}

/// The positional reconciler: common indices rebuild in place, a grown tail is
/// built, a shrunk tail is torn down and dropped. Length changes signal
/// `LAYOUT | PAINT`.
///
/// **Focus/capture survive a sibling structural change (Flutter's invariant).**
/// A structural edit among SIBLINGS must not clear focus/IME (or an in-flight
/// capture) for a child whose own identity is unchanged. Positional matching
/// rebuilds each common index `i` in place against the *same* live widget, so a
/// recorded focus/capture path to it stays valid as long as that slot was not an
/// in-place type swap. We therefore compute the **stable prefix** `k` — the
/// largest `k ≤ min(prev.len, next.len)` such that no index `< k` type-swapped
/// (the first swap index, or `common` if none) — and preserve focus AND capture
/// for pods `< k`. Only the tail from `k` onward has its recorded paths
/// cancelled/cleared via [`cancel_active_children`]: a swapped slot (fresh widget
/// that never saw `Down`), a shifted pod that may now hold different logical
/// content, and the grown tail (fresh pods, nothing to unwind). A truncated
/// active pod is cancelled earlier in [`teardown_child`]. A structural-change-free
/// rebuild (same length, no swap) leaves every path untouched, so an ordinary
/// every-frame rebuild never breaks a captured drag *or* dismisses the keyboard
/// for an unchanged sibling (the huddle search-field bug this fixes).
///
/// This is Flutter's focus/IME retention behavior: only a child whose
/// identity actually changes loses focus.
/// Positional matching cannot distinguish a same-typed prepend from a
/// content-change-plus-append — an index `< k` that positionally kept its widget
/// but semantically moved keeps its recorded path (the documented positional
/// limitation; a caller wanting identity across reorders uses `keyed`).
fn rebuild_children_positional<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
) -> ChangeFlags {
    let mut flags = ChangeFlags::NONE;
    let common = prev.len().min(next.len());
    // The stable prefix ends at the first in-place type swap (or at `common` if
    // there is none): every index before it keeps the same live widget across
    // the rebuild, so a recorded focus/capture path to it stays valid.
    let mut first_swap: Option<usize> = None;
    for i in 0..common {
        let (child_flags, child_swapped) =
            rebuild_child_tracked(view_of(&prev[i]), view_of(&next[i]), &mut pods[i], ctx);
        flags |= child_flags;
        if child_swapped {
            // Fresh widget at this slot: drop the stale capture, nothing to cancel.
            pods[i].set_active(false);
            if first_swap.is_none() {
                first_swap = Some(i);
            }
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
    // On a structural change (length shift or in-place type swap), preserve the
    // stable prefix (`< k`, same widget identity) and cancel/clear only the tail
    // from `k` onward. On a pure length change with no swap, `k == common`, so the
    // prefix (every surviving common pod) is preserved and only the grown tail —
    // fresh pods with no recorded path — is "cancelled" (a no-op); a truncated
    // active pod was already cancelled in `teardown_child`.
    if prev.len() != next.len() || first_swap.is_some() {
        let k = first_swap.unwrap_or(common);
        // `ctx` carries the chain down to *this container*; each pod's own flag
        // is ANDed onto it inside.
        cancel_active_children(&mut pods[k..], ctx);
    }
    flags
}

/// The keyed reconciler: match old↔new children by [`ChildKey`] so reorders and
/// inserts preserve each surviving child's live widget (and thus its internal
/// state) instead of rebuilding whatever happens to sit at the same index.
///
/// Matched children are *relocated* into the new order — their `ChildPod` (boxed
/// widget + geometry) is moved, then rebuilt in place against its own previous
/// view. Unmatched new keys are built fresh; unmatched old keys are torn down
/// (with [`teardown_child`]'s cancel-if-active). A duplicate key (old or new)
/// trips a `debug_assert` and falls back to the positional path.
///
/// **A key-matched child keeps its focus/capture across a move (Flutter's
/// invariant).** A reorder or insert relocates a matched child's whole
/// `ChildPod` — including its `focused`/`active` bookkeeping — so its recorded
/// path stays valid: focus/capture routing scans for the pod by its flag
/// ([`ChildPod::is_focused`]/[`is_active`](ChildPod::is_active)), so a `Key`/`Ime`
/// event (or a captured `Move`/`Up`) still reaches the relocated child at its new
/// index with nothing to update in a parent index. Only a child whose identity
/// actually breaks loses its path: a torn-down (removed) key is cancelled in
/// [`teardown_child`], and a key reused for a different concrete type is a swap
/// (the old widget died inside `AnyView::rebuild`) whose stale `active`/`focused`
/// flags are dropped inline below. A same-keys, same-order rebuild is likewise a
/// content-only rebuild that leaves every path untouched.
fn rebuild_children_keyed<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
    key_of: impl Fn(&C) -> Option<ChildKey>,
) -> ChangeFlags {
    // Old key -> old index, flagging any duplicate.
    let mut old_by_key: HashMap<ChildKey, usize> = HashMap::with_capacity(prev.len());
    let mut duplicate = false;
    for (i, child) in prev.iter().enumerate() {
        let key = key_of(child).expect("all-keyed list checked by caller");
        if old_by_key.insert(key, i).is_some() {
            duplicate = true;
        }
    }
    // Duplicate new keys are equally ambiguous (two children claim one identity).
    let mut seen_new: HashSet<ChildKey> = HashSet::with_capacity(next.len());
    for child in next {
        let key = key_of(child).expect("all-keyed list checked by caller");
        if !seen_new.insert(key) {
            duplicate = true;
        }
    }
    if duplicate {
        debug_assert!(
            false,
            "keyed child list has duplicate keys; falling back to positional reconciliation"
        );
        return rebuild_children_positional(prev, next, pods, ctx, view_of);
    }

    // Take ownership of the old pods so matched ones can be relocated by `take`.
    let mut old_pods: Vec<Option<ChildPod>> = pods.drain(..).map(Some).collect();
    let mut new_pods: Vec<ChildPod> = Vec::with_capacity(next.len());
    let mut flags = ChangeFlags::NONE;

    for child in next {
        let key = key_of(child).expect("all-keyed list checked by caller");
        if let Some(&old_index) = old_by_key.get(&key) {
            let mut pod = old_pods[old_index]
                .take()
                .expect("each old key matches at most one new child (no duplicates)");
            let (child_flags, child_swapped) =
                rebuild_child_tracked(view_of(&prev[old_index]), view_of(child), &mut pod, ctx);
            flags |= child_flags;
            if child_swapped {
                // A key reused for a different concrete type: the old widget was
                // torn down inside AnyView::rebuild, so its identity broke — drop
                // the stale capture/focus paths (nothing armed to unwind). A
                // key-matched non-swap relocates its pod (and thus its recorded
                // focus/capture flags) intact, so no clearing happens there.
                pod.set_active(false);
                // The focused widget died inside the swap: the recorded focus
                // path is severed, so the root's session goes with it — but only
                // if this pod's link was on the LIVE chain; a stale flag under a
                // blurred ancestor owns no session (see `mark_orphan_if_live` and
                // `cancel_active_children`'s note).
                let link_focused = pod.is_focused();
                if link_focused {
                    pod.set_focused(false);
                }
                mark_orphan_if_live(ctx, link_focused);
            }
            new_pods.push(pod);
        } else {
            // A brand-new key: build a fresh child.
            new_pods.push(build_child(view_of(child), ctx));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
    }

    // Any old pod never taken is a removed key: tear it down (cancelling first if
    // it held an in-flight capture, and dropping its focus flag with it).
    for (old_index, slot) in old_pods.iter_mut().enumerate() {
        if let Some(mut pod) = slot.take() {
            teardown_child(view_of(&prev[old_index]), &mut pod, ctx);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
    }

    *pods = new_pods;
    flags
}

/// A field shape a container can hold children in, visitable in declaration
/// order — the traversal half of the [`visit_children!`] seam.
///
/// Implemented for [`ChildPod`] and, generically, for `Option<T>`/`Vec<T>`/
/// `[T]` over anything visitable, so the three shapes containers actually use
/// (`child: ChildPod`, `icon: Option<ChildPod>`, `children: Vec<ChildPod>`) are
/// covered by one impl each. A container holding pods inside its own row/slot
/// struct implements this for that struct and stays on the same seam.
pub trait VisitPods {
    /// Hand each pod this value holds to `visitor`, in declaration order.
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod));
}

impl VisitPods for ChildPod {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(self);
    }
}

impl<T: VisitPods> VisitPods for Option<T> {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        if let Some(inner) = self {
            inner.visit_pods(visitor);
        }
    }
}

impl<T: VisitPods> VisitPods for [T] {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for item in self {
            item.visit_pods(visitor);
        }
    }
}

impl<T: VisitPods> VisitPods for Vec<T> {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.as_slice().visit_pods(visitor);
    }
}

/// Implement [`Widget::visit_children`](frust_core::Widget::visit_children) for
/// the container whose `impl Widget` block this is written in, forwarding the
/// named fields in the order given.
///
/// The read-only introspection seam that makes a container's retained subtree
/// enumerable (`frust-core`'s `WidgetTree::inspect` is the consumer). One line
/// per container, so the traversal lives here rather than being re-hand-written
/// per widget:
///
/// ```
/// # use frust_core::{BoxConstraints, ChildPod, LayoutCtx, PaintCtx, PaintScene, Widget};
/// # use kurbo::Size;
/// use frust_widgets::authoring::visit_children;
///
/// struct RowWidget {
///     leading: Option<ChildPod>,
///     children: Vec<ChildPod>,
/// }
///
/// impl Widget for RowWidget {
///     # fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size { bc.max() }
///     # fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
///     // ...layout/paint/event/semantics as usual...
///     visit_children!(leading, children);
/// }
/// ```
///
/// Each field's type only has to implement [`VisitPods`] — `ChildPod`,
/// `Option<_>`, `Vec<_>`, or a container's own row struct. A widget whose
/// children are reached some other way (a slot list behind an enum, a
/// transition's stashed page) writes the method by hand instead; the contract is
/// the same either way — visit every pod you own, in paint order, and nothing
/// else.
#[macro_export]
macro_rules! visit_children {
    ($($field:ident),* $(,)?) => {
        fn visit_children(&self, visitor: &mut dyn FnMut(&$crate::authoring::ChildPod)) {
            $( $crate::authoring::VisitPods::visit_pods(&self.$field, visitor); )*
        }
    };
}

pub use crate::visit_children;
/// Re-exported so the [`visit_children!`] expansion can name the type without
/// the call site importing it.
pub use frust_core::ChildPod;

/// Whether `event` is the phase that auto-releases a recorded capture
/// (`Up`/`Cancel`) — shared by [`route_event`]/[`route_event_single`].
fn releases_capture(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p)
            if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
    )
}

/// Whether `event` is a pointer `Down` — the phase that both opens a capture and
/// triggers blur-on-outside-tap evaluation in the routing helpers.
fn is_pointer_down(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Down)
    )
}

/// Route a pointer/scroll/keyboard/IME event — or a broadcast — to a
/// container's children.
///
/// **Broadcasts** ([`InputEvent::is_broadcast`], today
/// [`InputEvent::Housekeeping`]) are checked **first**, ahead of the capture,
/// focus, and hit-test branches: every child receives one unconditionally, in
/// order, and the container always reports [`EventResult::Ignored`] so no
/// first-handler-wins short-circuit can hide a subtree. This is what carries a
/// deferred, state-bearing callback flush (a navigator's queued `on_result`) to
/// wherever it was queued, on the frame that queued it, with no user input — see
/// [`InputEvent::Housekeeping`].
///
/// **Focus-routed events** ([`InputEvent::Key`]/[`InputEvent::Ime`]) bypass hit
/// testing entirely: they go straight to the child holding the recorded focus
/// path ([`ChildPod::is_focused`]), or are ignored if none does. This is the
/// focus mirror of the capture fast-path.
///
/// For **pointer/scroll**: a captured gesture goes straight to the recorded
/// active child (capture auto-releases on `Up`/`Cancel`). Otherwise the children
/// are hit-tested in reverse paint order — topmost (last-painted) first — and the
/// first child that both contains the point and reports [`EventResult::Handled`]
/// consumes it. This is the z-order convention Flex establishes and Stack shares.
///
/// **Blur-on-outside-tap:** a pointer `Down` that does not (re)establish focus on
/// the child it hits clears every focused child in this container. At the nearest
/// common ancestor of a stale focus branch and the tapped branch, this breaks the
/// recorded focus chain (a `Down` inside the still-focused child keeps it — that
/// child stays focused and is not cleared).
///
/// **Deeper stale flags below a cleared link are harmless by construction**, not
/// merely "corrected later". Nothing reads a `focused` flag on its own: every
/// consumer composes it with the chain above it, so a cleared link forces the
/// whole subtree below to read as unfocused. Focus-routed events stop at the
/// cleared link (this function finds no focused child to forward to); paint ANDs
/// the same way (`ChildPod::paint_child`'s `self.focused && ctx.has_focus()`); and
/// the rebuild pass ANDs the same way too ([`BuildCtx::has_focus`]), so tearing a
/// stale branch down marks no orphan and cannot release the session of the field
/// that really is focused ([`mark_orphan_if_live`]). The flags themselves are
/// still cleaned up the next time focus enters that subtree (a focus request
/// re-records the whole chain).
pub fn route_event(
    children: &mut [ChildPod],
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if event.is_broadcast() {
        for pod in children.iter_mut() {
            // Results are discarded on purpose: a broadcast is never consumed,
            // so every child gets it regardless of what an earlier one returned.
            pod.event_child(ctx, event);
        }
        return EventResult::Ignored;
    }
    if event.is_focus_routed() {
        if let Some(pod) = children.iter_mut().find(|p| p.is_focused()) {
            return pod.event_child(ctx, event);
        }
        return EventResult::Ignored;
    }
    if let Some(pod) = children.iter_mut().find(|p| p.is_active()) {
        return route_event_single(pod, ctx, event);
    }
    let position = event.position();
    let mut handled = EventResult::Ignored;
    // The index of the hit child *if* it holds focus after dispatch — the one
    // focused child blur-on-Down must preserve.
    let mut kept_focus: Option<usize> = None;
    let n = children.len();
    for i in (0..n).rev() {
        if children[i].contains(position)
            && children[i].event_child(ctx, event) == EventResult::Handled
        {
            if children[i].is_focused() {
                kept_focus = Some(i);
            }
            handled = EventResult::Handled;
            break;
        }
    }
    if is_pointer_down(event) {
        for (i, pod) in children.iter_mut().enumerate() {
            if Some(i) != kept_focus && pod.is_focused() {
                pod.set_focused(false);
            }
        }
    }
    handled
}

/// Route a pointer/scroll event — or a broadcast — to a container's single
/// child.
///
/// Mirrors [`route_event`] for the one-child wrappers (`Padding`/`Align`/
/// `SizedBox`): a broadcast ([`InputEvent::is_broadcast`]) reaches the child
/// unconditionally and is never consumed (checked first, ahead of everything
/// else); a captured gesture is forwarded to `pod` unconditionally —
/// regardless of whether the event's position still falls within the child's
/// bounds — with the active path cleared on `Up`/`Cancel`; otherwise the child
/// only receives the event if it contains the point. Re-hit-testing
/// `pod.contains()` on every event instead of consulting [`ChildPod::is_active`]
/// is the bug this helper exists to prevent — see `ChildPod::contains`'s docs.
pub fn route_event_single(
    pod: &mut ChildPod,
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if event.is_broadcast() {
        // Never consumed: forward, discard the result.
        pod.event_child(ctx, event);
        return EventResult::Ignored;
    }
    if event.is_focus_routed() {
        // Focus-routed events go to the child only if it holds the focus path.
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
    // Blur-on-outside-tap: a `Down` that lands *outside* the (single) focused
    // child drops its recorded focus path. A `Down` inside the child keeps focus
    // (the child re-requests it, or simply stays the focused widget).
    if is_pointer_down(event) && !inside && pod.is_focused() {
        pod.set_focused(false);
    }
    result
}

/// Compile-time proof that the whole authoring toolkit is reachable through the
/// **public** `crate::authoring::` path, not just the crate-root re-export the
/// in-crate call sites use.
///
/// Every item is named through `crate::authoring::…` and bound to an explicit
/// type, so an accidental re-privatization (or a signature change) fails the
/// build here rather than silently breaking a downstream design-system crate.
/// The module doc's worked example is the companion check: rustdoc compiles it as
/// a genuinely external crate.
#[cfg(test)]
mod authoring_surface_tests {
    use frust_core::{AnyView, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent};

    #[test]
    // The point of every binding below is the spelled-out type: it is what makes
    // a re-privatization or a signature change fail the build here.
    #[allow(clippy::type_complexity)]
    fn every_promoted_item_is_reachable_through_the_authoring_path() {
        // Callback erasure: the two adapters, the typed input, the two erasers.
        let _erased: Option<crate::authoring::ErasedCallback> = None;
        let _erased_arg: Option<crate::authoring::ErasedArgCallback<f64>> = None;
        let _typed: Option<crate::authoring::TypedArgCallback<(), f64>> = None;
        let _erase: fn(&std::rc::Rc<dyn Fn(&mut ())>) -> crate::authoring::ErasedCallback =
            crate::authoring::erase_callback::<()>;
        let _erase_arg: fn(
            &crate::authoring::TypedArgCallback<(), f64>,
        ) -> crate::authoring::ErasedArgCallback<f64> =
            crate::authoring::erase_callback_arg::<(), f64>;

        // Child plumbing: build/rebuild/teardown one child, reconcile many.
        let _build: fn(&AnyView<()>, &mut BuildCtx<'_>) -> ChildPod =
            crate::authoring::build_child::<()>;
        let _rebuild: fn(
            &AnyView<()>,
            &AnyView<()>,
            &mut ChildPod,
            &mut BuildCtx<'_>,
        ) -> ChangeFlags = crate::authoring::rebuild_child::<()>;
        let _teardown: fn(&AnyView<()>, &mut ChildPod, &mut BuildCtx<'_>) =
            crate::authoring::teardown_child::<()>;
        // `rebuild_children` takes `impl Fn` closures, which cannot be
        // turbofished into a `fn` pointer like the others — call it instead
        // (empty lists: the point is the path, not the reconciliation).
        let mut counter = 0u64;
        let mut build_ctx = BuildCtx::new(&mut counter);
        let mut pods: Vec<ChildPod> = Vec::new();
        let empty: Vec<AnyView<()>> = Vec::new();
        crate::authoring::rebuild_children(
            &empty,
            &empty,
            &mut pods,
            &mut build_ctx,
            |v: &AnyView<()>| v,
            |_: &AnyView<()>| None::<crate::ChildKey>,
        );
        assert!(pods.is_empty());

        // Event routing: multi-child and single-child.
        let _route: fn(&mut [ChildPod], &mut EventCtx<'_>, &InputEvent) -> EventResult =
            crate::authoring::route_event;
        let _route_single: fn(&mut ChildPod, &mut EventCtx<'_>, &InputEvent) -> EventResult =
            crate::authoring::route_event_single;

        // Themed text roles: the enum here, the builder method on `TextView`.
        let _role: crate::authoring::ThemeTextColor = crate::authoring::ThemeTextColor::OnSurface;
        let _themed_role: fn(crate::TextView, crate::authoring::ThemeTextColor) -> crate::TextView =
            crate::TextView::themed_role;

        // The shared press-overlay opacity.
        let _pressed: f32 = crate::authoring::PRESSED_OPACITY;

        // Child visitation: the trait, its three shape impls, and the macro
        // that writes the `Widget::visit_children` body from them.
        let _visit: fn(&ChildPod, &mut dyn FnMut(&ChildPod)) =
            <ChildPod as crate::authoring::VisitPods>::visit_pods;
        let _visit_opt: fn(&Option<ChildPod>, &mut dyn FnMut(&ChildPod)) =
            <Option<ChildPod> as crate::authoring::VisitPods>::visit_pods;
        let _visit_vec: fn(&Vec<ChildPod>, &mut dyn FnMut(&ChildPod)) =
            <Vec<ChildPod> as crate::authoring::VisitPods>::visit_pods;
        // The macro's own reachability is proven by its doc example, which
        // rustdoc compiles as a genuinely external crate.
    }
}

/// Mechanism-level tests for [`rebuild_child`]'s type-swap capture *and focus*
/// handling, plus the orphan-marking sites of the multi-child reconcilers:
/// every single-child container (`Padding`/`Align`/
/// `SizedBox`, interactive widgets' labels) reconciles its child through this
/// helper, so the clear-on-swap behavior is proven once here at the shared
/// substrate, and each container's own test module (see `padding`/`align`/
/// `sized`) proves it end to end through real event routing/geometry.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{leaf_any, swap_leaf};
    use frust_core::{BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, PaintScene, any};
    use kurbo::Size;

    /// A context on the **live** focus chain — what a diff under a real focused
    /// root sees (`BuildCtx::new`'s "unknown, assume live" default).
    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    /// A context whose focus chain was broken by an ancestor: the diff of a
    /// subtree hanging below a link a container-routed blur already cleared.
    fn blurred_ctx(counter: &mut u64) -> BuildCtx<'_> {
        let mut ctx = BuildCtx::new(counter);
        ctx.set_has_focus(false);
        ctx
    }

    /// A minimal multi-child container **view/widget pair** — the smallest thing
    /// that reconciles a child list through [`rebuild_children`] — so the focus
    /// chain can be exercised through genuine nesting (outer container → inner
    /// container → leaf) rather than one level of pods.
    struct NestView {
        children: Vec<AnyView<()>>,
    }

    struct NestWidget {
        children: Vec<ChildPod>,
    }

    impl View<()> for NestView {
        type Element = NestWidget;

        fn build(&self, ctx: &mut BuildCtx<'_>) -> NestWidget {
            NestWidget {
                children: self.children.iter().map(|c| build_child(c, ctx)).collect(),
            }
        }

        fn rebuild(
            &self,
            prev: &Self,
            element: &mut NestWidget,
            ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            rebuild_children(
                &prev.children,
                &self.children,
                &mut element.children,
                ctx,
                |v: &AnyView<()>| v,
                |_: &AnyView<()>| None::<ChildKey>,
            )
        }

        fn teardown(&self, element: &mut NestWidget, ctx: &mut BuildCtx<'_>) {
            for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
                teardown_child(view, pod, ctx);
            }
        }
    }

    impl Widget for NestWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// Reach the [`NestWidget`] inside a pod built from a [`NestView`] (double-boxed
    /// by [`build_child`]), so a test can set a pod flag deep in the tree.
    fn nest_in(pod: &mut ChildPod) -> &mut NestWidget {
        let boxed = pod
            .widget_mut()
            .downcast_mut::<Box<dyn Widget>>()
            .expect("pod holds a boxed AnyView widget");
        (**boxed)
            .downcast_mut::<NestWidget>()
            .expect("the boxed widget is the NestWidget")
    }

    #[test]
    fn rebuild_child_clears_active_on_type_swap() {
        // A type swap (Leaf -> SwapLeaf, different concrete types) tears down
        // the old widget inside AnyView::rebuild and builds a fresh one in its
        // place; a still-active pod must have its stale capture dropped so a
        // later event isn't misrouted into the fresh widget.
        let mut counter = 0u64;
        let prev: AnyView<()> = leaf_any(10.0, 10.0);
        let mut pod = build_child(&prev, &mut ctx(&mut counter));
        pod.set_active(true);

        let next: AnyView<()> = swap_leaf().into_any();
        rebuild_child::<()>(&prev, &next, &mut pod, &mut ctx(&mut counter));

        assert!(
            !pod.is_active(),
            "a type-swapped child's stale capture must be dropped"
        );
    }

    #[test]
    fn rebuild_child_preserves_active_on_same_type_rebuild() {
        // Negative guard against over-clearing: a same-type, content-only
        // rebuild (no AnyView type swap) must NOT touch an in-flight capture —
        // mirrors rebuild_children's content_only_rebuild_preserves_captured_drag.
        let mut counter = 0u64;
        let prev: AnyView<()> = leaf_any(10.0, 10.0);
        let mut pod = build_child(&prev, &mut ctx(&mut counter));
        pod.set_active(true);

        let next: AnyView<()> = leaf_any(20.0, 20.0);
        rebuild_child::<()>(&prev, &next, &mut pod, &mut ctx(&mut counter));

        assert!(
            pod.is_active(),
            "a content-only rebuild must not clear an in-flight capture"
        );
    }

    /// [`rebuild_child`]'s type-swap marking site — the single-child wrappers
    /// (`Padding`/`Align`/`SizedBox`/`GestureDetector`/`Scroll`, the card/dialog
    /// slot helpers, `ListView`'s surviving rows) — pinned in both directions.
    /// `Padding(if editing { text_input } else { text })` is the shipped shape:
    /// the focused widget dies inside the swap, so a live session dies with it,
    /// while the same swap over a stale flag under a blurred ancestor owns no
    /// session to lose.
    #[test]
    fn a_wrapper_type_swap_marks_only_on_a_live_chain() {
        fn swap_wrapped_child(build: &mut dyn FnMut(&mut u64) -> BuildCtx<'_>) -> bool {
            let _ = frust_core::take_focus_orphaned();
            let mut counter = 0u64;
            let prev: AnyView<()> = leaf_any(10.0, 10.0);
            let mut pod = build_child(&prev, &mut ctx(&mut counter));
            // The wrapper's single child holds the recorded focus path.
            pod.set_focused(true);

            // Leaf -> SwapLeaf: a concrete-type change, so the old widget dies
            // inside `AnyView::rebuild` and a fresh one takes its pod.
            let next: AnyView<()> = swap_leaf().into_any();
            rebuild_child::<()>(&prev, &next, &mut pod, &mut build(&mut counter));
            assert!(
                !pod.is_focused(),
                "a swapped-in widget must not inherit the old focus link"
            );
            frust_core::take_focus_orphaned()
        }

        assert!(
            swap_wrapped_child(&mut ctx),
            "a live focus path dying inside a wrapper's type swap releases the session"
        );
        assert!(
            !swap_wrapped_child(&mut blurred_ctx),
            "the same swap over a stale flag below a cleared link marks nothing"
        );
    }

    /// The negative guard for the site above: a same-type, content-only rebuild
    /// through a wrapper keeps the focus path (Flutter's retention invariant),
    /// so it must neither clear the flag nor mark — a mark here would drop the
    /// keyboard on every frame a padded field re-renders.
    #[test]
    fn a_wrapper_content_only_rebuild_keeps_focus_and_marks_nothing() {
        let _ = frust_core::take_focus_orphaned();
        let mut counter = 0u64;
        let prev: AnyView<()> = leaf_any(10.0, 10.0);
        let mut pod = build_child(&prev, &mut ctx(&mut counter));
        pod.set_focused(true);

        let next: AnyView<()> = leaf_any(20.0, 20.0);
        rebuild_child::<()>(&prev, &next, &mut pod, &mut ctx(&mut counter));

        assert!(
            pod.is_focused(),
            "a content-only rebuild must not clear the recorded focus path"
        );
        assert!(
            !frust_core::take_focus_orphaned(),
            "an unchanged wrapper child must not orphan the focus session"
        );
    }

    /// The producer half of the generic-unmount focus release: a reconciler that
    /// severs a recorded focus path raises the orphan mark, which
    /// `RenderRoot::rebuild` drains into a full session release (the consumer
    /// half is pinned in `frust-core`'s
    /// `generic_unmount_orphan_releases_the_whole_session`).
    #[test]
    fn a_truncated_focused_child_marks_the_focus_orphan() {
        let _ = frust_core::take_focus_orphaned();
        let mut counter = 0u64;
        let prev: Vec<AnyView<()>> = vec![leaf_any(10.0, 10.0), leaf_any(10.0, 10.0)];
        let mut pods: Vec<ChildPod> = prev
            .iter()
            .map(|v| build_child(v, &mut ctx(&mut counter)))
            .collect();
        // The second child holds the recorded focus path.
        pods[1].set_focused(true);
        assert!(
            !frust_core::take_focus_orphaned(),
            "building a list orphans nothing"
        );

        // Shrink the list: the focused child is torn down and dropped.
        let next: Vec<AnyView<()>> = vec![leaf_any(10.0, 10.0)];
        rebuild_children(
            &prev,
            &next,
            &mut pods,
            &mut ctx(&mut counter),
            |v: &AnyView<()>| v,
            |_: &AnyView<()>| None::<ChildKey>,
        );
        assert_eq!(pods.len(), 1);
        assert!(
            frust_core::take_focus_orphaned(),
            "the focused child's unmount must be reported to the render root"
        );
    }

    /// The cross-branch guard for the [`teardown_child`] marking site: the same
    /// truncation as above, but with the container's own link to the root already
    /// cleared (a blur moved focus to another branch, leaving this pod's flag
    /// stale). The stale pod owns no session, so its unmount must mark nothing —
    /// marking here is what dropped the keyboard out of an unrelated, still-live
    /// field when a list recycled a row.
    #[test]
    fn a_truncated_stale_focused_child_under_a_blurred_link_marks_nothing() {
        let _ = frust_core::take_focus_orphaned();
        let mut counter = 0u64;
        let prev: Vec<AnyView<()>> = vec![leaf_any(10.0, 10.0), leaf_any(10.0, 10.0)];
        let mut pods: Vec<ChildPod> = prev
            .iter()
            .map(|v| build_child(v, &mut ctx(&mut counter)))
            .collect();
        // Stale: the flag is set, but the chain above this container is broken.
        pods[1].set_focused(true);

        let next: Vec<AnyView<()>> = vec![leaf_any(10.0, 10.0)];
        rebuild_children(
            &prev,
            &next,
            &mut pods,
            &mut blurred_ctx(&mut counter),
            |v: &AnyView<()>| v,
            |_: &AnyView<()>| None::<ChildKey>,
        );
        assert_eq!(pods.len(), 1);
        assert!(
            !frust_core::take_focus_orphaned(),
            "a stale flag below a cleared link owns no session — releasing here \
             would kill the focus of whatever field really is focused"
        );
    }

    /// The [`cancel_active_children`] marking site (the cleared tail past the
    /// first type swap), pinned in both directions: it marks on a live chain and
    /// stays silent under a blurred one.
    #[test]
    fn a_cleared_tail_marks_only_on_a_live_chain() {
        // Index 0 type-swaps (Leaf -> SwapLeaf), so the tail from 0 onward has
        // its recorded paths cleared; index 1 holds the focus flag.
        fn shrink_tail(build: &mut dyn FnMut(&mut u64) -> BuildCtx<'_>) -> bool {
            let _ = frust_core::take_focus_orphaned();
            let mut counter = 0u64;
            let prev: Vec<AnyView<()>> = vec![leaf_any(10.0, 10.0), leaf_any(10.0, 10.0)];
            let mut pods: Vec<ChildPod> = prev
                .iter()
                .map(|v| build_child(v, &mut ctx(&mut counter)))
                .collect();
            pods[1].set_focused(true);
            let next: Vec<AnyView<()>> = vec![swap_leaf().into_any(), leaf_any(10.0, 10.0)];
            rebuild_children(
                &prev,
                &next,
                &mut pods,
                &mut build(&mut counter),
                |v: &AnyView<()>| v,
                |_: &AnyView<()>| None::<ChildKey>,
            );
            assert!(!pods[1].is_focused(), "the cleared tail drops the flag");
            frust_core::take_focus_orphaned()
        }

        assert!(
            shrink_tail(&mut ctx),
            "a live focus link cleared by the tail sweep releases the session"
        );
        assert!(
            !shrink_tail(&mut blurred_ctx),
            "the same sweep over a stale flag below a cleared link marks nothing"
        );
    }

    /// The keyed reconciler's type-swap marking site, pinned in both directions:
    /// a key reused for a different concrete type breaks identity, so a *live*
    /// focus link dies with it — but a stale one under a blurred ancestor does
    /// not.
    #[test]
    fn a_keyed_type_swap_marks_only_on_a_live_chain() {
        fn swap_keyed(build: &mut dyn FnMut(&mut u64) -> BuildCtx<'_>) -> bool {
            let _ = frust_core::take_focus_orphaned();
            let mut counter = 0u64;
            let key = ChildKey::new(1u32);
            let prev: Vec<(AnyView<()>, ChildKey)> = vec![(leaf_any(10.0, 10.0), key)];
            let mut pods: Vec<ChildPod> = prev
                .iter()
                .map(|c| build_child(&c.0, &mut ctx(&mut counter)))
                .collect();
            pods[0].set_focused(true);
            // Same key, different concrete view type: a swap, not a relocation.
            let next: Vec<(AnyView<()>, ChildKey)> = vec![(swap_leaf().into_any(), key)];
            rebuild_children(
                &prev,
                &next,
                &mut pods,
                &mut build(&mut counter),
                |c: &(AnyView<()>, ChildKey)| &c.0,
                |c: &(AnyView<()>, ChildKey)| Some(c.1),
            );
            assert!(!pods[0].is_focused(), "a swapped-away pod drops the flag");
            frust_core::take_focus_orphaned()
        }

        assert!(
            swap_keyed(&mut ctx),
            "a live focus path dying inside a keyed swap releases the session"
        );
        assert!(
            !swap_keyed(&mut blurred_ctx),
            "a stale flag dying inside a keyed swap releases nothing"
        );
    }

    /// The chain must compose through **nesting**, root → container → child: the
    /// blur clears the link at the nearest common ancestor (the outer container's
    /// pod), and the leaf's own flag two levels down stays stale. Tearing that
    /// leaf out must mark nothing — while the identical teardown with the outer
    /// link intact must still mark.
    #[test]
    fn the_focus_chain_composes_through_nested_containers() {
        fn drop_inner_leaf(outer_link_focused: bool) -> bool {
            let _ = frust_core::take_focus_orphaned();
            let mut counter = 0u64;
            let prev: Vec<AnyView<()>> = vec![any(NestView {
                children: vec![leaf_any(10.0, 10.0)],
            })];
            let mut pods: Vec<ChildPod> = prev
                .iter()
                .map(|v| build_child(v, &mut ctx(&mut counter)))
                .collect();
            // The leaf deep inside holds the focus flag either way; only the
            // OUTER link differs — that is the whole experiment.
            nest_in(&mut pods[0]).children[0].set_focused(true);
            pods[0].set_focused(outer_link_focused);

            // The inner container loses its only child (a filtered/recycled row).
            let next: Vec<AnyView<()>> = vec![any(NestView { children: vec![] })];
            rebuild_children(
                &prev,
                &next,
                &mut pods,
                &mut ctx(&mut counter),
                |v: &AnyView<()>| v,
                |_: &AnyView<()>| None::<ChildKey>,
            );
            assert!(
                nest_in(&mut pods[0]).children.is_empty(),
                "the inner child was torn down"
            );
            frust_core::take_focus_orphaned()
        }

        assert!(
            drop_inner_leaf(true),
            "an unbroken chain root→container→leaf is a live session; its unmount releases it"
        );
        assert!(
            !drop_inner_leaf(false),
            "a cleared link at the nearest common ancestor makes every flag below it inert"
        );
    }

    /// The negative guard: an ordinary content-only rebuild keeps the focus path
    /// (Flutter's retention invariant) and must therefore orphan nothing — a mark
    /// raised here would drop the keyboard on every frame.
    #[test]
    fn a_content_only_rebuild_marks_no_focus_orphan() {
        let _ = frust_core::take_focus_orphaned();
        let mut counter = 0u64;
        let prev: Vec<AnyView<()>> = vec![leaf_any(10.0, 10.0), leaf_any(10.0, 10.0)];
        let mut pods: Vec<ChildPod> = prev
            .iter()
            .map(|v| build_child(v, &mut ctx(&mut counter)))
            .collect();
        pods[1].set_focused(true);

        let next: Vec<AnyView<()>> = vec![leaf_any(20.0, 20.0), leaf_any(20.0, 20.0)];
        rebuild_children(
            &prev,
            &next,
            &mut pods,
            &mut ctx(&mut counter),
            |v: &AnyView<()>| v,
            |_: &AnyView<()>| None::<ChildKey>,
        );
        assert!(pods[1].is_focused(), "the focus path survives intact");
        assert!(
            !frust_core::take_focus_orphaned(),
            "an unchanged child must not orphan the focus session"
        );
    }
}

/// Focus-path routing tests for [`route_event`]/[`route_event_single`]: Key/IME
/// events reach only the focused child (including through a nested container),
/// blur-on-outside-tap breaks the chain, a second focus request moves focus,
/// `ApplyEditingState` routes to the focused child, a published IME surface
/// bubbles up, and the capture and focus paths stay independent.
#[cfg(test)]
mod focus_tests {
    use super::*;
    use frust_core::{
        BoxConstraints, EditingState, EventCtx, ImeEvent, ImeState, Key, KeyEvent, LayoutCtx,
        Modifiers, PaintCtx, PaintScene,
    };
    use kurbo::{Point, Rect, Size};

    /// A leaf that: on a pointer `Down` optionally requests focus and records
    /// `id + 100`; on a `Move` (capture path) records `id + 200`; on a Key/IME
    /// event records `id` and optionally publishes an IME surface. All state is a
    /// shared `Vec<u32>` so tests can assert *which* leaf saw *what*.
    struct KeyLeaf {
        id: u32,
        takes_focus: bool,
        publish_ime: bool,
    }

    impl Widget for KeyLeaf {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) => match p.phase {
                    PointerPhase::Down => {
                        if self.takes_focus {
                            ctx.request_focus();
                        }
                        ctx.state_mut::<Vec<u32>>().push(self.id + 100);
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        ctx.state_mut::<Vec<u32>>().push(self.id + 200);
                        EventResult::Handled
                    }
                    _ => EventResult::Handled,
                },
                InputEvent::Key(_) | InputEvent::Ime(_) => {
                    ctx.state_mut::<Vec<u32>>().push(self.id);
                    if self.publish_ime {
                        ctx.publish_ime_state(ImeState {
                            active: true,
                            editing: EditingState {
                                text: format!("leaf{}", self.id),
                                selection_base: 1,
                                selection_extent: 1,
                                composing_base: -1,
                                composing_extent: -1,
                            },
                            caret: Some(Rect::new(0.0, 0.0, 1.0, 10.0)),
                            content_type: Default::default(),
                        });
                    }
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    /// A minimal multi-child container: stacks its children vertically (each 10
    /// tall at the given width) and routes events through [`route_event`] — the
    /// same helper the real `Flex`/`Stack` use — so focus routing is exercised
    /// through a nested container.
    struct Nest {
        children: Vec<ChildPod>,
    }

    impl Widget for Nest {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            let w = bc.max().width;
            for (i, pod) in self.children.iter_mut().enumerate() {
                pod.set_origin(Point::new(0.0, i as f64 * 10.0));
                pod.layout_child(ctx, &BoxConstraints::tight(Size::new(w, 10.0)));
            }
            Size::new(w, self.children.len() as f64 * 10.0)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            route_event(&mut self.children, ctx, event)
        }
    }

    fn leaf_pod(id: u32, takes_focus: bool, publish_ime: bool) -> ChildPod {
        ChildPod::new(Box::new(KeyLeaf {
            id,
            takes_focus,
            publish_ime,
        }))
    }

    /// Lay out a slice of pods vertically (10 tall each at width 100) so hit
    /// testing distinguishes them: child `i` occupies `y ∈ [i*10, i*10+10)`.
    fn lay_out_vertically(pods: &mut [ChildPod]) {
        let mut lctx = LayoutCtx::new();
        for (i, pod) in pods.iter_mut().enumerate() {
            pod.set_origin(Point::new(0.0, i as f64 * 10.0));
            pod.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 10.0)));
        }
    }

    fn down(y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(5.0, y),
            button: PointerButton::Primary,
        })
    }

    fn mv(y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(5.0, y),
            button: PointerButton::Primary,
        })
    }

    fn key() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character("a".to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn ime_apply() -> InputEvent {
        InputEvent::Ime(ImeEvent::ApplyEditingState(EditingState {
            text: "sync".to_string(),
            selection_base: 4,
            selection_extent: 4,
            composing_base: -1,
            composing_extent: -1,
        }))
    }

    // `state` must be a concrete `&mut Vec<u32>`: it is erased to `&mut dyn Any`
    // and the leaves recover it with `state_mut::<Vec<u32>>()`, so a slice would
    // fail the downcast.
    #[allow(clippy::ptr_arg)]
    fn route(children: &mut [ChildPod], state: &mut Vec<u32>, event: &InputEvent) -> EventResult {
        let mut ctx = EventCtx::new(state, Point::ZERO, Size::new(100.0, 100.0));
        route_event(children, &mut ctx, event)
    }

    #[test]
    fn key_routes_only_to_focused_child() {
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        // Tap child 2 (y in [10,20)) → it requests focus.
        route(&mut children, &mut state, &down(15.0));
        assert!(children[1].is_focused());
        assert!(!children[0].is_focused());
        state.clear();

        // A Key event reaches ONLY the focused child (id 2), never child 1.
        route(&mut children, &mut state, &key());
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn key_reaches_focused_leaf_through_nested_container() {
        // Outer container holds one Nest; the Nest holds two leaves.
        let inner = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        let mut nest = Nest { children: inner };
        // Lay out the nest so its inner children get real geometry.
        {
            let mut lctx = LayoutCtx::new();
            nest.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 20.0)));
        }
        let mut outer = vec![ChildPod::new(Box::new(nest))];
        outer[0].set_origin(Point::ZERO);
        {
            let mut lctx = LayoutCtx::new();
            outer[0].layout_child(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 20.0)));
        }
        let mut state = Vec::new();

        // Tap the second inner leaf (y in [10,20)) → focus chain nest→leaf2.
        route(&mut outer, &mut state, &down(15.0));
        assert!(outer[0].is_focused(), "the nest records the focus path");
        state.clear();

        // Key routes outer→nest→leaf2 only.
        route(&mut outer, &mut state, &key());
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn blur_on_outside_tap_clears_focus_chain() {
        // Child 1 takes focus; child 2 does NOT (a non-editable widget).
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, false, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(5.0)); // focus child 1
        assert!(children[0].is_focused());

        // Tap child 2 (does not take focus) → blur clears the focus chain.
        route(&mut children, &mut state, &down(15.0));
        assert!(
            !children[0].is_focused(),
            "outside tap blurs the focused child"
        );
        assert!(!children[1].is_focused());
        state.clear();

        // A Key event now reaches nobody.
        let result = route(&mut children, &mut state, &key());
        assert_eq!(result, EventResult::Ignored);
        assert!(state.is_empty());
    }

    #[test]
    fn tap_inside_focused_widget_keeps_focus() {
        // A focused widget re-tapped keeps focus (Down inside focused widget).
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, false, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(5.0)); // focus child 1
        route(&mut children, &mut state, &down(5.0)); // tap child 1 again
        assert!(
            children[0].is_focused(),
            "a tap inside the focused widget keeps focus"
        );
        state.clear();
        route(&mut children, &mut state, &key());
        assert_eq!(state, vec![1]);
    }

    #[test]
    fn second_focus_request_moves_focus() {
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(5.0)); // focus child 1
        assert!(children[0].is_focused());
        route(&mut children, &mut state, &down(15.0)); // focus child 2
        assert!(children[1].is_focused());
        assert!(!children[0].is_focused(), "focus moved off child 1");
        state.clear();

        route(&mut children, &mut state, &key());
        assert_eq!(
            state,
            vec![2],
            "key now reaches only the newly focused child"
        );
    }

    #[test]
    fn ime_apply_routes_to_focused_child() {
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(15.0)); // focus child 2
        state.clear();
        route(&mut children, &mut state, &ime_apply());
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn capture_and_focus_paths_are_independent() {
        // Child 1 holds the capture (active) path; child 2 holds the focus path.
        let mut children = vec![leaf_pod(1, false, false), leaf_pod(2, false, false)];
        lay_out_vertically(&mut children);
        children[0].set_active(true);
        children[1].set_focused(true);
        let mut state = Vec::new();

        // A Move routes down the capture path → child 1 only (id+200).
        route(&mut children, &mut state, &mv(999.0));
        assert_eq!(state, vec![201]);
        state.clear();

        // A Key routes down the focus path → child 2 only (id).
        route(&mut children, &mut state, &key());
        assert_eq!(state, vec![2]);

        // Both paths survive intact.
        assert!(children[0].is_active());
        assert!(children[1].is_focused());
    }
}
