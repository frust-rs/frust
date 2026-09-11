//! The widget-author face of the overlay portal: where a floated surface is
//! placed, the slot a widget that hosts its own surface keeps, and the
//! declarative wrapper for the common case.
//!
//! [`frust_core::overlay`] owns the *mechanism* — a per-paint registry the
//! render root drains, paints above the whole main tree and hit-tests before
//! it. This module owns the three things every caller of that mechanism would
//! otherwise write for itself:
//!
//! * [`place`] — the anchored placement geometry (a side, a cross-axis
//!   alignment, a gap, a collision flip and a shift-back-inside clamp). Pure,
//!   total and usable on its own.
//! * [`OverlaySlot`] — the owner's half of the portal: it holds the pod, lays
//!   it out against the window, computes its window rect and registers it every
//!   paint, and routes the broadcast the root sends back into it. A widget that
//!   floats a surface of its own (a field hosting a selection toolbar, a menu
//!   button) holds one of these and forwards four calls to it.
//! * [`overlay_portal`] — the declarative wrapper for the common case: a child,
//!   an optional overlay view anchored to that child's bounds, and the portal
//!   does the rest.
//!
//! # Coordinate spaces
//!
//! Three spaces meet here, and every bug in a floating surface is a confusion
//! between two of them:
//!
//! * **Window space** — absolute logical pixels. The registered
//!   [`OverlayEntry::window_rect`](frust_core::OverlayEntry::window_rect) is in
//!   it, the root hit-tests in it, and the payload of an
//!   [`InputEvent::Overlay`] is in it. A widget only ever learns its own window
//!   position in `paint`, from
//!   [`PaintCtx::origin`](frust_core::PaintCtx::origin) — which is why the
//!   placement is computed there and nowhere else.
//! * **Owner-local space** — what the owner's own `event` sees: every container
//!   between the root and the owner has already subtracted its origin. An
//!   ordinary pointer event routed to the owner by a live capture arrives here,
//!   so the slot adds the owner origin it recorded at paint time to get back to
//!   window space.
//! * **Pod space** — the floated pod's own local space, which is window space
//!   minus the placed rect's origin. That single subtraction is the whole
//!   translation into the surface.
//!
//! # Not in v1
//!
//! * **The pod contributes no semantics.** A pod's nodes would attach under the
//!   owner's own accessibility node, at the owner's position rather than the
//!   floated rect's, so nothing is published rather than something wrong. An
//!   assistive-technology user reaches a floated surface through the owner
//!   (a field's own actions, a trigger's own node), not through the surface.
//! * **No hover inside a pod.** The root marks a hover pass on a hit-tested
//!   pointer event only, and an overlay event is a broadcast, so
//!   [`EventCtx::claim_hover`](frust_core::EventCtx::claim_hover) inside a
//!   floated surface records nothing.
//! * **A substituted pod publishes no IME surface** (see
//!   [`OverlaySlot::event`]); a pod over the ambient application state
//!   ([`OverlaySlot::event_ambient`], the route [`overlay_portal`] takes)
//!   publishes normally.
//! * **No focus trap and no nesting**, per [`frust_core::overlay`]'s own list.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, OutsideTap, OverlayBand, OverlayEntry, OverlayEventKind, OverlayInput, OverlayKey,
    PaintCtx, PaintScene, PointerEvent, PointerPhase, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Rect, Size};

use crate::authoring::{ErasedCallback, erase_callback, route_event_single};

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

/// The side of the anchor a floated surface opens on.
///
/// The web vocabulary every anchored-overlay pattern in this workspace already
/// speaks (Radix's `side`, and the four-sided tooltip variants each catalog
/// grew on top of it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlaySide {
    /// Above the anchor.
    Top,
    /// To the trailing side of the anchor.
    Right,
    /// Below the anchor — the default.
    #[default]
    Bottom,
    /// To the leading side of the anchor.
    Left,
}

impl OverlaySide {
    /// The side a collision flip lands on.
    pub const fn opposite(self) -> Self {
        match self {
            OverlaySide::Top => OverlaySide::Bottom,
            OverlaySide::Bottom => OverlaySide::Top,
            OverlaySide::Left => OverlaySide::Right,
            OverlaySide::Right => OverlaySide::Left,
        }
    }

    /// Whether this side stacks the surface vertically (`Top`/`Bottom`).
    pub const fn is_vertical(self) -> bool {
        matches!(self, OverlaySide::Top | OverlaySide::Bottom)
    }
}

/// How a floated surface lines up with its anchor on the cross axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayAlign {
    /// Leading edges flush (left edges for a `Top`/`Bottom` side, top edges for
    /// a `Left`/`Right` one).
    Start,
    /// Centres flush — the default.
    #[default]
    Center,
    /// Trailing edges flush.
    End,
}

/// A resolved placement request: which side, how it lines up, how far off the
/// anchor it sits, and how it may move to stay inside the area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayPlacement {
    /// The preferred side.
    pub side: OverlaySide,
    /// Cross-axis alignment.
    pub align: OverlayAlign,
    /// Gap between the anchor and the surface, in logical px.
    pub offset: f64,
    /// Flip to [`OverlaySide::opposite`] when the preferred side does not fit
    /// and the opposite one does.
    pub flip: bool,
    /// Shift the placed rect back inside the padded area when it overflows.
    pub clamp: bool,
    /// The margin the surface keeps from every edge of the area, in logical px.
    /// Both the fit test and the clamp read the area inset by it.
    pub padding: f64,
}

/// The default gap between an anchor and the surface placed against it, in
/// logical px.
///
/// The small neutral gap two of the three anchored hosts in this workspace
/// already default to; a design system that wants a wider one (a panel with a
/// visible neck) sets [`OverlayPlacement::offset`] in its own wrapper rather
/// than changing this.
pub const DEFAULT_OFFSET: f64 = 4.0;

/// The default margin a placed surface keeps from every edge of the area, in
/// logical px — the viewport padding the collision clamp works against.
pub const DEFAULT_PADDING: f64 = 8.0;

impl Default for OverlayPlacement {
    /// Below the anchor, centred, at [`DEFAULT_OFFSET`], flipping and clamping
    /// inside [`DEFAULT_PADDING`] of the area's edges.
    fn default() -> Self {
        OverlayPlacement {
            side: OverlaySide::default(),
            align: OverlayAlign::default(),
            offset: DEFAULT_OFFSET,
            flip: true,
            clamp: true,
            padding: DEFAULT_PADDING,
        }
    }
}

impl OverlayPlacement {
    /// A placement on `side`, everything else defaulted.
    pub fn on(side: OverlaySide) -> Self {
        OverlayPlacement {
            side,
            ..Self::default()
        }
    }

    /// Set the cross-axis alignment.
    pub const fn align(mut self, align: OverlayAlign) -> Self {
        self.align = align;
        self
    }

    /// Set the anchor gap, in logical px.
    pub const fn offset(mut self, offset: f64) -> Self {
        self.offset = offset;
        self
    }

    /// Enable or disable the collision flip.
    pub const fn flip(mut self, flip: bool) -> Self {
        self.flip = flip;
        self
    }

    /// Enable or disable the shift-back-inside clamp.
    pub const fn clamp(mut self, clamp: bool) -> Self {
        self.clamp = clamp;
        self
    }

    /// Set the margin kept from the area's edges, in logical px.
    pub const fn padding(mut self, padding: f64) -> Self {
        self.padding = padding;
        self
    }
}

/// The region a surface may occupy: `area` inset by `padding` on every side.
///
/// Falls back to `area` itself when the inset would invert it — a window
/// narrower than twice the padding still has to place its surface somewhere,
/// and an inverted rect would make the clamp below meaningless.
fn field(area: Rect, padding: f64) -> Rect {
    let inset = area.inset(-padding);
    if inset.width() > 0.0 && inset.height() > 0.0 {
        inset
    } else {
        area
    }
}

/// Whether a `content`-sized surface fits on `side` of `anchor` inside `field`.
fn fits(side: OverlaySide, anchor: Rect, content: Size, field: Rect, offset: f64) -> bool {
    match side {
        OverlaySide::Top => anchor.y0 - offset - content.height >= field.y0,
        OverlaySide::Bottom => anchor.y1 + offset + content.height <= field.y1,
        OverlaySide::Left => anchor.x0 - offset - content.width >= field.x0,
        OverlaySide::Right => anchor.x1 + offset + content.width <= field.x1,
    }
}

/// The cross-axis start coordinate for `align`, given the anchor's own span
/// `[a0, a1]` and the surface's `extent` along that axis.
fn align_start(align: OverlayAlign, a0: f64, a1: f64, extent: f64) -> f64 {
    match align {
        OverlayAlign::Start => a0,
        OverlayAlign::Center => (a0 + a1) / 2.0 - extent / 2.0,
        OverlayAlign::End => a1 - extent,
    }
}

/// Shift `rect` back inside `field`, keeping its size.
///
/// The start edge wins when the surface is larger than the field (`min` before
/// `max`): a too-wide panel hangs off the trailing edge rather than the leading
/// one, where its content starts.
fn clamp_into(rect: Rect, field: Rect) -> Rect {
    let x = rect.x0.min(field.x1 - rect.width()).max(field.x0);
    let y = rect.y0.min(field.y1 - rect.height()).max(field.y0);
    Rect::from_origin_size(Point::new(x, y), rect.size())
}

/// Place a `content`-sized surface against `anchor` inside `area`.
///
/// All three rects are in one coordinate space — window space, for every caller
/// inside this module. The returned rect is the surface's placed bounds:
///
/// 1. inset `area` by `placement.padding` — every step below works in that
///    field;
/// 2. pick the side: the preferred one, or its opposite when `flip` is on, the
///    preferred one does not fit and the opposite one does;
/// 3. offset off that edge of the anchor by `offset`, lined up on the cross axis
///    per `align`;
/// 4. shift back inside the field when `clamp` is on.
///
/// Pure and total: it allocates nothing, reads no context, and is defined for a
/// degenerate anchor (a zero-size rect places against that point) and for
/// content larger than the field (which pins to the field's *start* edge).
///
/// This is the one copy of geometry each anchored-overlay host in the design
/// systems had derived for itself (`plugins/shadcn`, `plugins/beui` and
/// `plugins/material`'s `overlay::anchored`), promoted here so a widget, a
/// catalog and an app all place a surface the same way.
pub fn place(anchor: Rect, content: Size, area: Rect, placement: OverlayPlacement) -> Rect {
    let field = field(area, placement.padding);
    let mut side = placement.side;
    if placement.flip
        && !fits(side, anchor, content, field, placement.offset)
        && fits(side.opposite(), anchor, content, field, placement.offset)
    {
        side = side.opposite();
    }
    let origin = match side {
        OverlaySide::Top => Point::new(
            align_start(placement.align, anchor.x0, anchor.x1, content.width),
            anchor.y0 - placement.offset - content.height,
        ),
        OverlaySide::Bottom => Point::new(
            align_start(placement.align, anchor.x0, anchor.x1, content.width),
            anchor.y1 + placement.offset,
        ),
        OverlaySide::Left => Point::new(
            anchor.x0 - placement.offset - content.width,
            align_start(placement.align, anchor.y0, anchor.y1, content.height),
        ),
        OverlaySide::Right => Point::new(
            anchor.x1 + placement.offset,
            align_start(placement.align, anchor.y0, anchor.y1, content.height),
        ),
    };
    let rect = Rect::from_origin_size(origin, content);
    if placement.clamp {
        clamp_into(rect, field)
    } else {
        rect
    }
}

// ---------------------------------------------------------------------------
// The owner's slot
// ---------------------------------------------------------------------------

/// What a slot places its surface against.
///
/// Both variants resolve to a **window-space** rect in the owner's `paint`,
/// where the owner's absolute origin is finally known; nothing is subscribed to
/// and nothing is cached across frames, so an anchor follows its owner across
/// scroll, relayout and animation for free.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OverlayAnchor {
    /// The owner's own bounds — a trigger floating a menu under itself.
    #[default]
    Owner,
    /// A rect in the **owner's local space**: a caret, a selection's bounding
    /// box, a press point, one row of a list. Translated by the owner's paint
    /// origin, so a caller states it in the same coordinates its `layout` and
    /// `event` already speak.
    Rect(Rect),
}

/// One floated surface an owner hosts: the pod, where it goes, and the routing
/// of the input the root sends back to it.
///
/// # The four calls
///
/// An owner widget forwards four of its own lifecycle calls here, and the slot
/// does nothing on its own:
///
/// 1. [`rebuild`](Self::rebuild) from the owner's `View::rebuild`, with the
///    overlay view it wants mounted (or `None` to drop it).
/// 2. [`layout`](Self::layout) from the owner's `Widget::layout` — the pod is
///    laid out loosely against the **window**, never the owner's own
///    constraints, because it escapes the owner's box entirely.
/// 3. [`paint`](Self::paint) from the owner's `Widget::paint`, which computes
///    the placement and registers the pod. It paints nothing: the root paints
///    every registered pod after the main tree, which is the only way a surface
///    escapes its owner's paint order and every ancestor's clip.
/// 4. [`event`](Self::event) (or [`event_ambient`](Self::event_ambient)) from
///    the owner's `Widget::event`, **before** the owner routes to its own
///    children. `None` means "not mine" and the owner carries on.
///
/// # `PodState`
///
/// The state type the floated view is diffed against. Two shapes exist and the
/// dispatch call differs between them:
///
/// * the pod is built over the **ambient application state** (what
///   [`overlay_portal`] does): use [`event_ambient`](Self::event_ambient), which
///   forwards through the owner's own [`EventCtx`] so focus, capture, hover and
///   IME all bubble exactly as they do for any other child;
/// * the pod is built over a **different** state — `()` for a
///   framework-built surface whose callbacks carry their own handles: use
///   [`event`](Self::event) and hand it `&mut PodState`, which dispatches over a
///   substituted context (see that method for what does and does not bubble
///   through the substitution).
pub struct OverlaySlot<PodState: 'static> {
    /// This surface's identity, allocated once and quoted back by every routed
    /// event. Stable for the slot's life — re-allocating per frame would hand
    /// the root a new identity every paint.
    key: OverlayKey,
    /// The mounted pod, shared with the registration the root paints. `None`
    /// while the surface is closed.
    pod: Option<Rc<RefCell<ChildPod>>>,
    band: OverlayBand,
    input: OverlayInput,
    outside_tap: OutsideTap,
    placement: OverlayPlacement,
    anchor: OverlayAnchor,
    /// The window size the last layout pass saw — the area the placement is
    /// computed against, recorded in `layout` because a paint context carries
    /// no window size of its own.
    window: Size,
    /// Where the last paint placed the surface, in window space: what was
    /// registered, what the root hit-tests, and what a routed position is made
    /// pod-local against.
    window_rect: Rect,
    /// The owner's own absolute origin as of the last paint — what an
    /// owner-local pointer position (a captured drag) is lifted into window
    /// space with.
    owner_origin: Point,
    /// Whether the pod holds the pointer capture, so ordinary pointer events
    /// routed to the owner by the capture path belong to the surface.
    captured: bool,
    /// A press landed outside every floated surface and this one asked to hear
    /// about it; drained by [`take_outside_down`](Self::take_outside_down).
    outside_down_pending: bool,
    _state: PhantomData<fn(&mut PodState)>,
}

impl<PodState: 'static> Default for OverlaySlot<PodState> {
    fn default() -> Self {
        Self::new()
    }
}

impl<PodState: 'static> OverlaySlot<PodState> {
    /// A closed slot with a fresh identity: `Floating`, interactive, ignoring
    /// outside taps, anchored to its owner's own bounds.
    ///
    /// Built **once**, when the owner widget is built, and kept: the key is the
    /// whole addressing mechanism between the root and this surface.
    pub fn new() -> Self {
        Self {
            key: OverlayKey::next(),
            pod: None,
            band: OverlayBand::Floating,
            input: OverlayInput::Interactive,
            outside_tap: OutsideTap::Ignore,
            placement: OverlayPlacement::default(),
            anchor: OverlayAnchor::Owner,
            window: Size::ZERO,
            window_rect: Rect::ZERO,
            owner_origin: Point::ZERO,
            captured: false,
            outside_down_pending: false,
            _state: PhantomData,
        }
    }

    /// This surface's identity.
    pub fn key(&self) -> OverlayKey {
        self.key
    }

    /// Whether a pod is currently mounted.
    pub fn is_open(&self) -> bool {
        self.pod.is_some()
    }

    /// Where the last paint placed the surface, in window space.
    ///
    /// [`Rect::ZERO`] before the first paint of an open slot — a surface that
    /// has never been painted has never been registered, so nothing routes to
    /// it either.
    pub fn window_rect(&self) -> Rect {
        self.window_rect
    }

    /// Whether the pod holds the recorded focus path.
    pub fn pod_has_focus(&self) -> bool {
        self.pod
            .as_ref()
            .is_some_and(|pod| pod.borrow().is_focused())
    }

    /// Drop the pod's recorded focus link, so focus-routed events stop reaching
    /// it — the owner's half of "the surface asked for focus, and the owner
    /// declined on its behalf".
    pub fn withdraw_pod_focus(&mut self) {
        if let Some(pod) = &self.pod {
            pod.borrow_mut().set_focused(false);
        }
    }

    /// Take the pending outside-press notification, clearing it.
    ///
    /// `true` exactly once per press that landed outside every floated surface
    /// while this one was registered [`OutsideTap::Notify`] — the light-dismiss
    /// signal an owner closes on.
    pub fn take_outside_down(&mut self) -> bool {
        std::mem::take(&mut self.outside_down_pending)
    }

    /// Which z-band the surface paints and hit-tests in.
    pub fn set_band(&mut self, band: OverlayBand) {
        self.band = band;
    }

    /// Whether the surface takes pointer input at all.
    pub fn set_input(&mut self, input: OverlayInput) {
        self.input = input;
    }

    /// What a press outside every floated surface delivers here.
    pub fn set_outside_tap(&mut self, outside_tap: OutsideTap) {
        self.outside_tap = outside_tap;
    }

    /// Where the surface sits relative to its anchor.
    pub fn set_placement(&mut self, placement: OverlayPlacement) {
        self.placement = placement;
    }

    /// What the surface is placed against.
    pub fn set_anchor(&mut self, anchor: OverlayAnchor) {
        self.anchor = anchor;
    }

    /// Mount, reconcile or drop the floated view — the owner's `View::rebuild`
    /// half.
    ///
    /// `prev`/`next` are the previous and current frame's overlay views, in the
    /// shape [`rebuild_child`](crate::authoring::rebuild_child) itself takes: a
    /// `None` → `Some` transition builds the pod, `Some` → `Some` reconciles it
    /// in place (so a kept-open surface keeps its own widget state), `Some` →
    /// `None` tears it down, and `None` → `None` does nothing. An owner that
    /// mounts a view it builds itself (from a process-global builder, say)
    /// keeps the previous one and hands both in.
    ///
    /// Dropping the pod also drops any capture it held: the widget that was
    /// mid-gesture no longer exists, so there is nothing to unwind and nothing
    /// to route follow-ups to.
    pub fn rebuild(
        &mut self,
        prev: Option<&AnyView<PodState>>,
        next: Option<&AnyView<PodState>>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        match (prev, next) {
            (Some(prev), Some(next)) => match &self.pod {
                Some(pod) => {
                    let mut pod = pod.borrow_mut();
                    crate::authoring::rebuild_child(prev, next, &mut pod, ctx)
                }
                // The view stayed mounted but the pod did not: build a fresh
                // one rather than route into nothing.
                None => self.mount(next, ctx),
            },
            (None, Some(next)) => {
                // Nothing to reconcile against — an existing pod here belongs to
                // a view the owner no longer has, so it is replaced rather than
                // diffed (a torn-down widget's state dies with it either way).
                self.drop_pod();
                self.mount(next, ctx)
            }
            (Some(prev), None) => match self.pod.take() {
                Some(pod) => {
                    {
                        let mut pod = pod.borrow_mut();
                        crate::authoring::teardown_child(prev, &mut pod, ctx);
                    }
                    self.captured = false;
                    ChangeFlags::LAYOUT
                }
                None => ChangeFlags::NONE,
            },
            (None, None) => {
                if self.pod.is_some() {
                    self.drop_pod();
                    ChangeFlags::LAYOUT
                } else {
                    ChangeFlags::NONE
                }
            }
        }
    }

    /// Build `view` into a fresh pod, replacing whatever was mounted.
    fn mount(&mut self, view: &AnyView<PodState>, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let pod = crate::authoring::build_child(view, ctx);
        self.pod = Some(Rc::new(RefCell::new(pod)));
        self.captured = false;
        ChangeFlags::LAYOUT
    }

    /// Drop the pod with no view to tear it down through — the recovery arm for
    /// a slot whose mounted view vanished without one.
    fn drop_pod(&mut self) {
        self.pod = None;
        self.captured = false;
    }

    /// Lay the pod out against the window — the owner's `Widget::layout` half.
    ///
    /// Loose constraints against
    /// [`LayoutCtx::window_size`](frust_core::LayoutCtx::window_size), never the
    /// owner's own `bc`: the surface escapes the owner's box, so the owner's
    /// constraints say nothing about how much room it has. The pod's own origin
    /// stays [`Point::ZERO`] — the root paints it at the registered rect's
    /// origin and adds the pod's origin on top, and routing tests the registered
    /// rect alone, so any other value would desynchronize paint from hit test.
    pub fn layout(&mut self, ctx: &mut LayoutCtx) {
        self.window = ctx.window_size();
        if let Some(pod) = &self.pod {
            let bc = BoxConstraints::loose(self.window);
            let mut pod = pod.borrow_mut();
            pod.layout_child(ctx, &bc);
            pod.set_origin(Point::ZERO);
        }
    }

    /// Place and register the pod — the owner's `Widget::paint` half.
    ///
    /// Computes the anchor rect in window space from
    /// [`PaintCtx::origin`](frust_core::PaintCtx::origin) (plus the local rect,
    /// for [`OverlayAnchor::Rect`]), places the pod against it with [`place`],
    /// records the result and hands the root a registration. **It paints
    /// nothing**: an owner that also painted the pod would draw the surface
    /// twice, once clipped in place and once floated.
    ///
    /// Registration is per paint pass, so a surface stays alive exactly while
    /// its owner keeps painting — an owner that is culled, unmounted or simply
    /// stops registering disappears from the routing table after the next paint
    /// with nothing to unregister.
    pub fn paint(&mut self, ctx: &mut PaintCtx, owner_size: Size) {
        let Some(pod) = &self.pod else {
            return;
        };
        self.owner_origin = ctx.origin();
        let anchor = match self.anchor {
            OverlayAnchor::Owner => Rect::from_origin_size(ctx.origin(), owner_size),
            OverlayAnchor::Rect(local) => local + ctx.origin().to_vec2(),
        };
        let area = Rect::from_origin_size(Point::ZERO, self.window);
        let content = pod.borrow().size();
        self.window_rect = place(anchor, content, area, self.placement);
        ctx.register_overlay(OverlayEntry {
            key: self.key,
            band: self.band,
            input: self.input,
            outside_tap: self.outside_tap,
            window_rect: self.window_rect,
            pod: Rc::clone(pod),
        });
    }

    /// Route an event into the surface over a **substituted** state — the
    /// owner's `Widget::event` half for a pod whose `PodState` is not the
    /// ambient application state (a `()`-typed, framework-built surface).
    ///
    /// `Some(_)` means the slot owned the event and the owner must not route it
    /// on; `None` means it belongs to the owner's ordinary routing.
    ///
    /// # What crosses the substitution
    ///
    /// The pod runs over a fresh [`EventCtx`] built on `state`, exactly as a
    /// component boundary runs its subtree over its own local state, and the
    /// results are mirrored back onto the owner's context: a redraw request, a
    /// pointer capture, and a focus claim or release (observed through the pod's
    /// own recorded link). A published IME surface and a hover claim do **not**
    /// cross — an IME publish has no route back through a substituted context,
    /// and an overlay event is a broadcast, which records no hover anywhere. A
    /// surface that needs either is built over the ambient state instead (see
    /// [`event_ambient`](Self::event_ambient)).
    ///
    /// Edit commands do cross, by a different road: they ride a pass-scoped
    /// queue rather than the context, so a pod that calls
    /// [`EventCtx::dispatch_edit_command`](frust_core::EventCtx::dispatch_edit_command)
    /// is drained by the owner's
    /// [`EventCtx::take_edit_commands`](frust_core::EventCtx::take_edit_commands)
    /// in the same pass.
    pub fn event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &InputEvent,
        state: &mut PodState,
    ) -> Option<EventResult> {
        let substitute: &mut dyn Any = state;
        self.route(ctx, event, Some(substitute))
    }

    /// Route an event into the surface over the **ambient** application state —
    /// the owner's `Widget::event` half for a pod built over the same state the
    /// owner itself is diffed against.
    ///
    /// The pod is dispatched through the owner's own [`EventCtx`], so
    /// everything a child normally bubbles (redraw, capture, focus, a published
    /// IME surface) reaches the root unchanged, and the pod's callbacks reach
    /// the same application state every other widget sees. This is the route
    /// [`overlay_portal`] takes.
    pub fn event_ambient(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &InputEvent,
    ) -> Option<EventResult> {
        self.route(ctx, event, None)
    }

    /// The shared body of [`event`](Self::event)/
    /// [`event_ambient`](Self::event_ambient): decide whether this event belongs
    /// to the surface and, if it does, translate it into pod space and forward
    /// it.
    fn route(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &InputEvent,
        substitute: Option<&mut dyn Any>,
    ) -> Option<EventResult> {
        // A closed slot owns nothing: every event belongs to the owner.
        self.pod.as_ref()?;
        let origin = self.window_rect.origin().to_vec2();
        match event {
            // A floated surface's own input, broadcast to the whole tree so it
            // reaches this owner wherever it sits. The key comparison is the
            // entire addressing mechanism: another owner's surface falls
            // through untouched.
            InputEvent::Overlay(overlay) if overlay.key == self.key => {
                match &overlay.kind {
                    OverlayEventKind::Pointer(pointer) => {
                        let local = InputEvent::Pointer(PointerEvent {
                            position: pointer.position - origin,
                            ..*pointer
                        });
                        self.forward(ctx, &local, substitute);
                    }
                    OverlayEventKind::Scroll { position, delta } => {
                        let local = InputEvent::Scroll {
                            position: *position - origin,
                            delta: *delta,
                        };
                        self.forward(ctx, &local, substitute);
                    }
                    // The press landed on nothing floated: the surface never saw
                    // it, so nothing is forwarded — the owner reads the
                    // notification and decides whether to close.
                    OverlayEventKind::OutsideDown => self.outside_down_pending = true,
                }
                // A broadcast is never consumed, whatever the pod returned.
                Some(EventResult::Ignored)
            }
            // A gesture that began inside the surface: the capture it opened
            // short-circuits the root's overlay pre-pass, so its follow-ups
            // arrive here as ordinary pointer events in the OWNER's local space
            // and have to be lifted back into window space first.
            InputEvent::Pointer(pointer) if self.captured => {
                let local = InputEvent::Pointer(PointerEvent {
                    position: pointer.position + self.owner_origin.to_vec2() - origin,
                    ..*pointer
                });
                let result = self.forward(ctx, &local, substitute);
                if matches!(pointer.phase, PointerPhase::Up | PointerPhase::Cancel) {
                    self.captured = false;
                }
                Some(result)
            }
            // Keyboard, IME and the clipboard verbs: focus-routed, so they only
            // arrive here at all because the owner is on the recorded focus
            // chain — and they belong to the surface exactly when the surface is
            // what claimed focus.
            event if event.is_focus_routed() && self.pod_has_focus() => {
                Some(self.forward(ctx, event, substitute))
            }
            _ => None,
        }
    }

    /// Dispatch `local` (already in pod space) into the pod, updating the
    /// slot's own capture bookkeeping and, for a substituted state, mirroring
    /// the pod's results onto the owner's context.
    fn forward(
        &mut self,
        ctx: &mut EventCtx<'_>,
        local: &InputEvent,
        substitute: Option<&mut dyn Any>,
    ) -> EventResult {
        let Some(pod) = self.pod.clone() else {
            return EventResult::Ignored;
        };
        let mut pod = pod.borrow_mut();
        let was_active = pod.is_active();
        let was_focused = pod.is_focused();
        let result = match substitute {
            // The ambient route: the pod is a child like any other, and
            // `event_child` bubbles its redraw/capture/focus/IME flags into the
            // owner's context on its own.
            None => pod.event_child(ctx, local),
            Some(state) => {
                let (result, needs_redraw) = {
                    let mut inner =
                        EventCtx::new(state, self.window_rect.origin(), self.window_rect.size());
                    let result = pod.event_child(&mut inner, local);
                    (result, inner.needs_redraw())
                };
                if needs_redraw {
                    ctx.request_redraw();
                }
                // Capture and focus are mirrored through the pod's own recorded
                // links rather than the substituted context's flags: the pod
                // records exactly what its widget asked for, and reading it here
                // keeps one mirror instead of two.
                if pod.is_active() && !was_active {
                    ctx.capture_pointer();
                }
                match (was_focused, pod.is_focused()) {
                    (false, true) => ctx.request_focus(),
                    (true, false) => ctx.release_focus(),
                    _ => {}
                }
                result
            }
        };
        if pod.is_active() && !was_active {
            self.captured = true;
        }
        result
    }
}

// ---------------------------------------------------------------------------
// The declarative portal
// ---------------------------------------------------------------------------

/// Float `overlay` above the whole app, anchored to `child`'s bounds — the
/// declarative half of the portal.
///
/// The child is laid out, painted and routed exactly as it would be without the
/// wrapper: the portal adds nothing to its geometry and consumes none of its
/// input. The overlay is mounted while [`OverlayPortalView::overlay`] is
/// `Some`, which is also how an exit animation is expressed — keep handing the
/// same view in while the surface ramps out, and hand `None` once it has.
///
/// ```
/// use frust_core::any;
/// use frust_widgets::{OverlayPlacement, OverlaySide, overlay_portal, text};
///
/// struct App {
///     hovering: bool,
/// }
///
/// fn tip(state: &mut App) -> impl frust_core::View<App> + use<> {
///     overlay_portal(text("save"))
///         .overlay(state.hovering.then(|| any(text("Save the document"))))
///         .placement(OverlayPlacement::on(OverlaySide::Top))
/// }
/// # let _ = tip;
/// ```
pub fn overlay_portal<State: 'static, V: View<State>>(child: V) -> OverlayPortalView<State> {
    OverlayPortalView {
        child: any(child),
        overlay: None,
        placement: OverlayPlacement::default(),
        band: OverlayBand::Floating,
        input: OverlayInput::Interactive,
        outside_tap: OutsideTap::Ignore,
        on_outside_tap: None,
        preserve_focus: false,
    }
}

/// The light-dismiss callback a portal holds before it is erased.
type OnOutsideTap<State> = Rc<dyn Fn(&mut State)>;

/// A declarative overlay portal. See [`overlay_portal`].
pub struct OverlayPortalView<State: 'static> {
    child: AnyView<State>,
    overlay: Option<AnyView<State>>,
    placement: OverlayPlacement,
    band: OverlayBand,
    input: OverlayInput,
    outside_tap: OutsideTap,
    on_outside_tap: Option<OnOutsideTap<State>>,
    preserve_focus: bool,
}

impl<State: 'static> OverlayPortalView<State> {
    /// The floated surface: `Some` mounts it, `None` drops it after the next
    /// paint.
    ///
    /// The view is diffed against the **same** application state the portal
    /// itself is, so the surface reads and writes app state exactly like the
    /// child does — it is a logical child of this call site that happens to be
    /// painted elsewhere.
    pub fn overlay(mut self, overlay: Option<AnyView<State>>) -> Self {
        self.overlay = overlay;
        self
    }

    /// Where the surface sits relative to the child's bounds (default: below,
    /// centred — see [`OverlayPlacement::default`]).
    pub fn placement(mut self, placement: OverlayPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Which z-band the surface paints and hit-tests in (default
    /// [`OverlayBand::Floating`]; a tooltip belongs in
    /// [`OverlayBand::Tooltip`]).
    pub fn band(mut self, band: OverlayBand) -> Self {
        self.band = band;
        self
    }

    /// Whether the surface takes pointer input (default
    /// [`OverlayInput::Interactive`]; explanatory chrome the pointer passes
    /// through is [`OverlayInput::Transparent`]).
    pub fn input(mut self, input: OverlayInput) -> Self {
        self.input = input;
        self
    }

    /// What a press landing outside every floated surface delivers here
    /// (default [`OutsideTap::Ignore`]).
    pub fn outside_tap(mut self, outside_tap: OutsideTap) -> Self {
        self.outside_tap = outside_tap;
        self
    }

    /// Run `callback` when a press lands outside every floated surface — the
    /// light-dismiss hook.
    ///
    /// Setting it also opts the surface into the notification
    /// ([`OutsideTap::Notify`] with `consume: true`, the modal shape) unless an
    /// explicit [`outside_tap`](Self::outside_tap) says otherwise, so the tap
    /// that dismisses a menu does not also activate what sits under it. Pass
    /// `OutsideTap::Notify { consume: false }` for the pass-through shape.
    pub fn on_outside_tap(mut self, callback: impl Fn(&mut State) + 'static) -> Self {
        self.on_outside_tap = Some(Rc::new(callback));
        if self.outside_tap == OutsideTap::Ignore {
            self.outside_tap = OutsideTap::Notify { consume: true };
        }
        self
    }

    /// Keep the child's focus session when the surface claims focus (default
    /// `false`).
    ///
    /// A surface that is *about* the child rather than a place to type — a
    /// selection toolbar over a field — must not take the field's focus, or the
    /// selection it acts on disappears the moment it is touched. With this set,
    /// a focus claim from inside the surface is withdrawn again whenever the
    /// child held the focus link, and focus-routed events keep reaching the
    /// child. Left `false`, a surface that claims focus gets it and the child's
    /// link inside this portal is dropped, which is what a popover containing
    /// its own text field wants.
    pub fn preserve_focus(mut self, preserve: bool) -> Self {
        self.preserve_focus = preserve;
        self
    }
}

/// The retained widget for an [`OverlayPortalView`].
pub struct OverlayPortalWidget<State: 'static> {
    child: ChildPod,
    slot: OverlaySlot<State>,
    on_outside_tap: Option<ErasedCallback>,
    preserve_focus: bool,
}

impl<State: 'static> OverlayPortalView<State> {
    /// Push the view's routing configuration onto the slot — the half of
    /// `build`/`rebuild` that is identical in both.
    fn configure(&self, slot: &mut OverlaySlot<State>) {
        slot.set_placement(self.placement);
        slot.set_band(self.band);
        slot.set_input(self.input);
        slot.set_outside_tap(self.outside_tap);
    }
}

impl<State: 'static> View<State> for OverlayPortalView<State> {
    type Element = OverlayPortalWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayPortalWidget<State> {
        let mut slot = OverlaySlot::new();
        self.configure(&mut slot);
        slot.rebuild(None, self.overlay.as_ref(), ctx);
        OverlayPortalWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            slot,
            on_outside_tap: self.on_outside_tap.as_ref().map(erase_callback),
            preserve_focus: self.preserve_focus,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayPortalWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.placement != self.placement
            || prev.band != self.band
            || prev.input != self.input
            || prev.outside_tap != self.outside_tap
        {
            // Placement and the routing fields are re-read from the slot on the
            // next paint, which is also when the new rect is registered.
            flags |= ChangeFlags::PAINT;
        }
        self.configure(&mut element.slot);
        element.preserve_focus = self.preserve_focus;
        // Closures are not comparable, so the adapter is reinstalled
        // unconditionally — it is cheap.
        element.on_outside_tap = self.on_outside_tap.as_ref().map(erase_callback);
        flags |= element
            .slot
            .rebuild(prev.overlay.as_ref(), self.overlay.as_ref(), ctx);
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut OverlayPortalWidget<State>, ctx: &mut BuildCtx<'_>) {
        element.slot.rebuild(self.overlay.as_ref(), None, ctx);
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl<State: 'static> Widget for OverlayPortalWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        // The floated pod is sized against the window, not against `bc` — it
        // escapes this widget's box entirely.
        self.slot.layout(ctx);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
        // Registered, never painted here: the root paints it after the whole
        // main tree, which is what puts it above a later sibling.
        let size = ctx.size();
        self.slot.paint(ctx, size);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let child_focused = self.child.is_focused();
        // A focus-routed event belongs to the child whenever the child holds
        // the link. Both links can be set at once — `preserve_focus` leaves the
        // child's standing, and a child that takes focus back after the surface
        // had it claims its own without anything clearing the surface's, since
        // an overlay pod is not a sibling the root's blur rule can reach. The
        // main tree wins in both cases: the surface's link is the older one.
        if !(event.is_focus_routed() && child_focused) {
            let pod_focused_before = self.slot.pod_has_focus();
            if let Some(result) = self.slot.event_ambient(ctx, event) {
                // Drained unconditionally: a notification with no callback
                // installed is still spent, not left standing for a later pass.
                if self.slot.take_outside_down()
                    && let Some(callback) = &mut self.on_outside_tap
                {
                    callback(ctx);
                    ctx.request_redraw();
                }
                if !pod_focused_before && self.slot.pod_has_focus() {
                    if self.preserve_focus && child_focused {
                        // Hand the session back to the child: the surface's own
                        // claim is withdrawn, and re-asserting the request keeps
                        // every ancestor's recorded chain pointing here.
                        self.slot.withdraw_pod_focus();
                        ctx.request_focus();
                    } else {
                        // The surface took the session, so the child's link
                        // inside this portal is stale — an overlay press never
                        // reaches the root's own blur rule to clear it.
                        self.child.set_focused(false);
                    }
                }
                return result;
            }
        }
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The child only: a floated pod's nodes would attach at this widget's
        // position rather than the surface's (see the module docs).
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingScene;
    use crate::{Column, SizedBox, Stack, StackView, scroll_view};
    use frust_core::{
        EditCommand, FrameTime, Key, KeyEvent, Modifiers, NamedKey, PointerButton, RenderRoot,
        ScrollDelta,
    };
    use peniko::Color;

    // -----------------------------------------------------------------------
    // Placement
    // -----------------------------------------------------------------------

    const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 600.0);

    /// Placement with the padding switched off, so a geometry assertion reads
    /// against the raw area rather than the inset one.
    fn bare() -> OverlayPlacement {
        OverlayPlacement::default().padding(0.0)
    }

    fn anchor_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::from_origin_size(Point::new(x, y), Size::new(w, h))
    }

    #[test]
    fn the_default_placement_is_below_the_anchor_centred_at_the_neutral_gap() {
        let p = OverlayPlacement::default();
        assert_eq!(p.side, OverlaySide::Bottom);
        assert_eq!(p.align, OverlayAlign::Center);
        assert_eq!(p.offset, DEFAULT_OFFSET);
        assert_eq!(p.padding, DEFAULT_PADDING);
        assert!(p.flip && p.clamp);
    }

    #[test]
    fn each_side_offsets_off_its_own_edge() {
        let a = anchor_rect(100.0, 200.0, 80.0, 40.0);
        let content = Size::new(120.0, 60.0);
        // The raw side placement, with the collision passes off: `Left` would
        // otherwise flip (a 120px panel does not fit in the 100px to the
        // anchor's left), which the flip test below covers on its own.
        let raw = |side| OverlayPlacement::on(side).flip(false).clamp(false);
        let bottom = place(a, content, AREA, raw(OverlaySide::Bottom));
        assert_eq!(bottom.y0, a.y1 + DEFAULT_OFFSET);
        let top = place(a, content, AREA, raw(OverlaySide::Top));
        assert_eq!(top.y1, a.y0 - DEFAULT_OFFSET);
        let right = place(a, content, AREA, raw(OverlaySide::Right));
        assert_eq!(right.x0, a.x1 + DEFAULT_OFFSET);
        let left = place(a, content, AREA, raw(OverlaySide::Left));
        assert_eq!(left.x1, a.x0 - DEFAULT_OFFSET);
        // Sizes are never altered by placement.
        for r in [bottom, top, right, left] {
            assert_eq!(r.size(), content);
        }
        assert!(OverlaySide::Top.is_vertical() && !OverlaySide::Left.is_vertical());
        assert_eq!(OverlaySide::Top.opposite(), OverlaySide::Bottom);
        assert_eq!(OverlaySide::Left.opposite(), OverlaySide::Right);
    }

    #[test]
    fn align_lines_up_leading_center_or_trailing_edges() {
        let a = anchor_rect(100.0, 200.0, 80.0, 40.0);
        let content = Size::new(120.0, 60.0);
        let at = |side, align| place(a, content, AREA, OverlayPlacement::on(side).align(align));
        assert_eq!(at(OverlaySide::Bottom, OverlayAlign::Start).x0, a.x0);
        assert_eq!(at(OverlaySide::Bottom, OverlayAlign::End).x1, a.x1);
        assert_eq!(
            at(OverlaySide::Bottom, OverlayAlign::Center).center().x,
            a.center().x
        );

        // The same three on a horizontal side act on the vertical axis.
        assert_eq!(at(OverlaySide::Right, OverlayAlign::Start).y0, a.y0);
        assert_eq!(at(OverlaySide::Right, OverlayAlign::End).y1, a.y1);
        assert_eq!(
            at(OverlaySide::Right, OverlayAlign::Center).center().y,
            a.center().y
        );
    }

    #[test]
    fn a_side_that_does_not_fit_flips_to_the_opposite_one() {
        // An anchor near the bottom edge: `Bottom` overflows, `Top` fits.
        let a = anchor_rect(100.0, 560.0, 80.0, 20.0);
        let content = Size::new(120.0, 100.0);
        let flipped = place(a, content, AREA, bare());
        assert_eq!(
            flipped.y1,
            a.y0 - DEFAULT_OFFSET,
            "flipped above the anchor"
        );

        // With the flip disabled it stays below and only the clamp moves it.
        let pinned = place(a, content, AREA, bare().flip(false));
        assert_eq!(pinned.y1, AREA.y1, "clamped, not flipped");

        // Neither side fits: the preferred one is kept (and clamped).
        let tall = Size::new(120.0, 590.0);
        let kept = place(a, tall, AREA, bare());
        assert_eq!(kept.y1, AREA.y1);
    }

    #[test]
    fn each_side_flips_at_the_edge_it_would_overflow() {
        let content = Size::new(120.0, 100.0);
        // Top edge: a `Top` placement flips down.
        let high = anchor_rect(100.0, 10.0, 80.0, 20.0);
        let down = place(high, content, AREA, OverlayPlacement::on(OverlaySide::Top));
        assert_eq!(down.y0, high.y1 + DEFAULT_OFFSET);
        // Left edge: a `Left` placement flips right.
        let leading = anchor_rect(10.0, 200.0, 20.0, 20.0);
        let right = place(
            leading,
            content,
            AREA,
            OverlayPlacement::on(OverlaySide::Left),
        );
        assert_eq!(right.x0, leading.x1 + DEFAULT_OFFSET);
        // Right edge: a `Right` placement flips left.
        let trailing = anchor_rect(370.0, 200.0, 20.0, 20.0);
        let left = place(
            trailing,
            content,
            AREA,
            OverlayPlacement::on(OverlaySide::Right),
        );
        assert_eq!(left.x1, trailing.x0 - DEFAULT_OFFSET);
    }

    #[test]
    fn clamping_shifts_the_rect_back_inside_and_can_be_turned_off() {
        // An anchor at the right edge, centre-aligned: the panel overflows.
        let a = anchor_rect(380.0, 100.0, 20.0, 20.0);
        let content = Size::new(200.0, 50.0);
        let clamped = place(a, content, AREA, bare());
        assert_eq!(clamped.x1, AREA.x1);
        assert_eq!(
            clamped.y0,
            a.y1 + DEFAULT_OFFSET,
            "only the cross axis moved"
        );

        let free = place(a, content, AREA, bare().clamp(false));
        assert!(free.x1 > AREA.x1, "unclamped placement may overflow");

        // Content wider than the area pins to the leading edge (the start-edge
        // rule), not the trailing one.
        let huge = Size::new(600.0, 50.0);
        let pinned = place(a, huge, AREA, bare());
        assert_eq!(pinned.x0, AREA.x0);
    }

    #[test]
    fn the_clamp_keeps_the_padding_off_every_edge() {
        let content = Size::new(200.0, 50.0);
        // Trailing overflow lands `DEFAULT_PADDING` short of the area edge.
        let trailing = place(
            anchor_rect(380.0, 100.0, 20.0, 20.0),
            content,
            AREA,
            OverlayPlacement::default(),
        );
        assert_eq!(trailing.x1, AREA.x1 - DEFAULT_PADDING);
        // …and so does a leading one.
        let leading = place(
            anchor_rect(0.0, 100.0, 20.0, 20.0),
            content,
            AREA,
            OverlayPlacement::default(),
        );
        assert_eq!(leading.x0, AREA.x0 + DEFAULT_PADDING);
        // The bottom edge is the same rule on the other axis.
        let low = place(
            anchor_rect(100.0, 560.0, 20.0, 20.0),
            Size::new(100.0, 300.0),
            AREA,
            OverlayPlacement::default().flip(false),
        );
        assert_eq!(low.y1, AREA.y1 - DEFAULT_PADDING);
    }

    #[test]
    fn a_padding_larger_than_the_area_falls_back_to_the_area_itself() {
        // A window narrower than twice the padding still has to place its
        // surface somewhere; the inset would invert, so it is dropped.
        let tiny = Rect::new(0.0, 0.0, 10.0, 10.0);
        let rect = place(
            Rect::ZERO,
            Size::new(4.0, 4.0),
            tiny,
            OverlayPlacement::default().flip(false),
        );
        assert!(tiny.contains(rect.origin()));
        assert_eq!(rect.x0, 0.0, "clamped to the un-inset area's own edge");
    }

    #[test]
    fn a_degenerate_anchor_places_against_that_point() {
        let content = Size::new(80.0, 40.0);
        let rect = place(Rect::ZERO, content, AREA, OverlayPlacement::default());
        assert_eq!(
            rect.x0,
            AREA.x0 + DEFAULT_PADDING,
            "clamped in from the centred overhang"
        );
        // The neutral gap is smaller than the padding, so the same clamp holds
        // the surface off the top edge as well.
        assert_eq!(rect.y0, AREA.y0 + DEFAULT_PADDING);
    }

    // -----------------------------------------------------------------------
    // The portal, driven through a real render root
    // -----------------------------------------------------------------------

    /// Everything the fixture's widgets record, in the order it happened.
    type Log = Rc<RefCell<Vec<String>>>;

    const WINDOW: Size = Size::new(400.0, 600.0);

    /// Where the `Plain` fixture's surface lands: the top-left corner of the
    /// padded field, because a window-sized anchor fits its content on neither
    /// side and the clamp takes over.
    const POD: Rect = Rect::new(8.0, 8.0, 88.0, 38.0);

    /// What the floated content does when it is pressed.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    enum Reaction {
        #[default]
        Nothing,
        Capture,
        Focus,
    }

    /// Which tree the fixture builds.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Shape {
        /// A window-filling portal child with a later, window-filling sibling.
        Plain,
        /// A small portal child partway down a scrollable column.
        Scrolled,
    }

    #[derive(Clone, Copy)]
    struct Cfg {
        open: bool,
        shape: Shape,
        band: OverlayBand,
        input: OverlayInput,
        /// `None` leaves the policy to `on_outside_tap`'s own default.
        outside_tap: Option<OutsideTap>,
        preserve_focus: bool,
        reaction: Reaction,
        placement: OverlayPlacement,
    }

    /// The corner placement the `Plain` fixture pins its surface with — chosen
    /// so the rect is the same however the fixture is configured.
    fn corner() -> OverlayPlacement {
        OverlayPlacement::on(OverlaySide::Top)
            .offset(0.0)
            .align(OverlayAlign::Start)
    }

    impl Default for Cfg {
        fn default() -> Self {
            Cfg {
                open: true,
                shape: Shape::Plain,
                band: OverlayBand::Floating,
                input: OverlayInput::Interactive,
                outside_tap: Some(OutsideTap::Ignore),
                preserve_focus: false,
                reaction: Reaction::Nothing,
                placement: corner(),
            }
        }
    }

    struct App {
        cfg: Cfg,
        log: Log,
        presses: u32,
        outside_taps: u32,
    }

    /// A recording leaf: paints one rect at its own absolute origin, records
    /// every event it receives (positions in its own local space) and reacts to
    /// a press the way the fixture asked it to.
    struct Probe {
        tag: &'static str,
        /// `None` fills whatever it is offered.
        size: Option<Size>,
        reaction: Reaction,
        /// Whether its pointer arms report `Handled` — a `false` leaf lets a
        /// press fall through to the child painted under it.
        handles: bool,
        log: Log,
    }

    struct ProbeWidget {
        tag: &'static str,
        size: Option<Size>,
        reaction: Reaction,
        handles: bool,
        log: Log,
    }

    impl View<App> for Probe {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget {
                tag: self.tag,
                size: self.size,
                reaction: self.reaction,
                handles: self.handles,
                log: Rc::clone(&self.log),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.size = self.size;
            element.reaction = self.reaction;
            element.handles = self.handles;
            element.log = Rc::clone(&self.log);
            ChangeFlags::NONE
        }
    }

    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size.unwrap_or_else(|| bc.max()))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) => {
                    self.log.borrow_mut().push(format!(
                        "{}:{:?}@{},{}",
                        self.tag, p.phase, p.position.x, p.position.y
                    ));
                    if p.phase == PointerPhase::Down {
                        ctx.state_mut::<App>().presses += 1;
                        match self.reaction {
                            // A press this leaf only records: the commonest
                            // shape, and the one that must not disturb focus.
                            Reaction::Nothing => {}
                            Reaction::Capture => ctx.capture_pointer(),
                            Reaction::Focus => ctx.request_focus(),
                        }
                    }
                    if self.handles {
                        EventResult::Handled
                    } else {
                        EventResult::Ignored
                    }
                }
                InputEvent::Key(_) => {
                    self.log.borrow_mut().push(format!(
                        "{}:key focus={}",
                        self.tag,
                        ctx.has_focus()
                    ));
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    /// The fixture's whole view tree, rebuilt from the app state every frame so
    /// a test can reconfigure it between frames.
    fn logic(state: &mut App) -> StackView<App> {
        let cfg = state.cfg;
        let log = Rc::clone(&state.log);
        let pod = cfg.open.then(|| {
            any(Probe {
                tag: "pod",
                size: Some(Size::new(80.0, 30.0)),
                reaction: cfg.reaction,
                handles: true,
                log: Rc::clone(&log),
            })
        });
        // The child claims focus on a press, so it stands in for the focused
        // field a surface is opened over.
        let child = Probe {
            tag: "child",
            size: match cfg.shape {
                Shape::Plain => None,
                Shape::Scrolled => Some(Size::new(120.0, 40.0)),
            },
            reaction: Reaction::Focus,
            handles: true,
            log: Rc::clone(&log),
        };
        let mut portal = overlay_portal(child)
            .overlay(pod)
            .placement(cfg.placement)
            .band(cfg.band)
            .input(cfg.input)
            .preserve_focus(cfg.preserve_focus)
            .on_outside_tap(|state: &mut App| state.outside_taps += 1);
        if let Some(policy) = cfg.outside_tap {
            portal = portal.outside_tap(policy);
        }
        match cfg.shape {
            Shape::Plain => Stack(vec![
                any(portal),
                // Painted after the portal and covering it, so "the pod paints
                // above a later sibling" is a real question. It handles nothing,
                // so a press falls through to the portal's own child.
                any(Probe {
                    tag: "sib",
                    size: None,
                    reaction: Reaction::Nothing,
                    handles: false,
                    log,
                }),
            ]),
            Shape::Scrolled => Stack(vec![any(scroll_view(Column(vec![
                any(SizedBox(Some(400.0), Some(200.0))),
                any(portal),
                any(SizedBox(Some(400.0), Some(1000.0))),
            ])))]),
        }
    }

    struct Harness {
        root: RenderRoot<App, StackView<App>>,
        state: App,
        clock_ms: f64,
    }

    impl Harness {
        fn new(cfg: Cfg) -> Self {
            Harness {
                root: RenderRoot::new(),
                state: App {
                    cfg,
                    log: Rc::new(RefCell::new(Vec::new())),
                    presses: 0,
                    outside_taps: 0,
                },
                clock_ms: 0.0,
            }
        }

        /// One whole frame: rebuild, layout, paint — returning what was painted,
        /// in paint order.
        fn frame(&mut self) -> RecordingScene {
            let mut app_logic: fn(&mut App) -> StackView<App> = logic;
            self.root.rebuild(&mut app_logic, &mut self.state);
            self.root.layout(WINDOW);
            self.clock_ms += 16.0;
            let mut scene = RecordingScene::default();
            self.root.paint(
                &mut scene,
                FrameTime::from_nanos((self.clock_ms * 1_000_000.0) as u64),
            );
            scene
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }

        fn down(&mut self, x: f64, y: f64) {
            self.pointer(PointerPhase::Down, x, y);
        }

        fn key(&mut self) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::ArrowLeft),
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }

        fn log(&self) -> Vec<String> {
            self.state.log.borrow().clone()
        }

        fn clear_log(&mut self) {
            self.state.log.borrow_mut().clear();
        }
    }

    /// The rects a scene recorded, so a paint-order assertion reads as geometry.
    fn rects(scene: &RecordingScene) -> Vec<Rect> {
        scene
            .rects
            .iter()
            .map(|(origin, size)| Rect::from_origin_size(*origin, *size))
            .collect()
    }

    #[test]
    fn the_overlay_pod_paints_above_a_later_sibling() {
        let mut h = Harness::new(Cfg::default());
        let scene = h.frame();
        let painted = rects(&scene);
        assert_eq!(
            painted.last().copied(),
            Some(POD),
            "the floated pod paints after the whole main tree, not in place: {painted:?}"
        );
        // …and it really is covered in the main tree: the sibling painted over
        // the same pixels one step earlier.
        let sibling = painted[painted.len() - 2];
        assert_eq!(sibling, Rect::from_origin_size(Point::ZERO, WINDOW));
    }

    #[test]
    fn a_press_inside_the_surface_reaches_the_pod_in_its_own_space() {
        let mut h = Harness::new(Cfg::default());
        h.frame();
        h.clear_log();
        h.down(40.0, 20.0);
        assert_eq!(
            h.log(),
            vec!["pod:Down@32,12".to_string()],
            "the press is delivered in pod space (window minus the placed origin)"
        );
        assert_eq!(
            h.state.presses, 1,
            "the pod's own callback ran on app state"
        );
    }

    #[test]
    fn a_transparent_tooltip_surface_never_receives_input() {
        let mut h = Harness::new(Cfg {
            band: OverlayBand::Tooltip,
            input: OverlayInput::Transparent,
            ..Cfg::default()
        });
        let scene = h.frame();
        assert_eq!(
            rects(&scene).last().copied(),
            Some(POD),
            "a transparent surface is still painted above everything"
        );
        h.clear_log();
        h.down(40.0, 20.0);
        assert_eq!(
            h.log(),
            vec!["sib:Down@40,20".to_string(), "child:Down@40,20".to_string()],
            "the pointer passes through to the main tree, topmost sibling first"
        );
    }

    #[test]
    fn a_drag_begun_on_the_surface_continues_into_it_through_the_capture() {
        // Deliberately the scrolled tree: the owner sits 200px down, so the
        // follow-ups — which arrive in the OWNER's local space, not the
        // window's — are wrong by exactly that much unless they are lifted.
        let mut h = Harness::new(Cfg {
            shape: Shape::Scrolled,
            placement: OverlayPlacement::default(),
            reaction: Reaction::Capture,
            ..Cfg::default()
        });
        h.frame();
        h.clear_log();
        h.down(30.0, 250.0);
        assert!(
            h.root.is_pointer_captured(),
            "a capture claimed from inside the surface is honoured"
        );
        // Far outside the placed rect, and outside the owner too: the capture,
        // not the hit test, is what routes these.
        h.pointer(PointerPhase::Move, 200.0, 400.0);
        h.pointer(PointerPhase::Up, 210.0, 410.0);
        assert_eq!(
            h.log(),
            vec![
                "pod:Down@10,6".to_string(),
                "pod:Move@180,156".to_string(),
                "pod:Up@190,166".to_string(),
            ]
        );
        assert!(!h.root.is_pointer_captured(), "the Up releases the capture");
        // The gesture is over: a further move outside the surface is nobody's.
        h.clear_log();
        h.pointer(PointerPhase::Move, 200.0, 400.0);
        assert!(
            !h.log().iter().any(|e| e.starts_with("pod:")),
            "a move after the release is no longer the surface's: {:?}",
            h.log()
        );
    }

    #[test]
    fn a_press_outside_notifies_the_owner_and_can_still_reach_the_main_tree() {
        let mut h = Harness::new(Cfg {
            outside_tap: Some(OutsideTap::Notify { consume: false }),
            ..Cfg::default()
        });
        h.frame();
        h.clear_log();
        h.down(200.0, 400.0);
        assert_eq!(h.state.outside_taps, 1, "the owner was told");
        assert_eq!(
            h.log(),
            vec![
                "sib:Down@200,400".to_string(),
                "child:Down@200,400".to_string()
            ],
            "a pass-through notification still lets the press through"
        );
    }

    #[test]
    fn a_consuming_outside_press_is_swallowed() {
        let mut h = Harness::new(Cfg {
            outside_tap: Some(OutsideTap::Notify { consume: true }),
            ..Cfg::default()
        });
        h.frame();
        h.clear_log();
        h.down(200.0, 400.0);
        assert_eq!(h.state.outside_taps, 1);
        assert!(
            h.log().is_empty(),
            "the dismissing press never reached the main tree: {:?}",
            h.log()
        );
    }

    #[test]
    fn an_outside_tap_callback_opts_into_the_notification_by_itself() {
        // No explicit policy: installing the callback is what asks to hear.
        let mut h = Harness::new(Cfg {
            outside_tap: None,
            ..Cfg::default()
        });
        h.frame();
        h.clear_log();
        h.down(200.0, 400.0);
        assert_eq!(h.state.outside_taps, 1);
        assert!(
            h.log().is_empty(),
            "the implied policy is the modal one (consuming): {:?}",
            h.log()
        );
    }

    #[test]
    fn an_ignoring_surface_hears_nothing_about_an_outside_press() {
        let mut h = Harness::new(Cfg::default());
        h.frame();
        h.clear_log();
        h.down(200.0, 400.0);
        assert_eq!(h.state.outside_taps, 0);
        assert_eq!(
            h.log(),
            vec![
                "sib:Down@200,400".to_string(),
                "child:Down@200,400".to_string()
            ]
        );
    }

    #[test]
    fn a_surface_that_claims_focus_takes_it_from_the_child() {
        let mut h = Harness::new(Cfg {
            reaction: Reaction::Focus,
            preserve_focus: false,
            ..Cfg::default()
        });
        h.frame();
        // The child is focused first, exactly as a field is before its surface
        // opens over it.
        h.down(200.0, 400.0);
        assert!(h.root.is_focus_active());
        h.clear_log();
        h.key();
        assert_eq!(h.log(), vec!["child:key focus=true".to_string()]);

        // A press inside the surface, which claims focus for itself.
        h.clear_log();
        h.down(40.0, 20.0);
        h.key();
        assert_eq!(
            h.log(),
            vec![
                "pod:Down@32,12".to_string(),
                "pod:key focus=true".to_string()
            ],
            "the keyboard follows the surface"
        );
        assert!(h.root.is_focus_active());
    }

    #[test]
    fn preserve_focus_hands_the_session_back_to_the_child() {
        let mut h = Harness::new(Cfg {
            reaction: Reaction::Focus,
            preserve_focus: true,
            ..Cfg::default()
        });
        h.frame();
        h.down(200.0, 400.0);
        h.clear_log();
        h.down(40.0, 20.0);
        h.key();
        assert_eq!(
            h.log(),
            vec![
                "pod:Down@32,12".to_string(),
                "child:key focus=true".to_string()
            ],
            "the field that opened the surface keeps typing"
        );
        assert!(h.root.is_focus_active(), "and keeps its session");
    }

    #[test]
    fn a_child_that_takes_focus_back_gets_the_keyboard_again() {
        let mut h = Harness::new(Cfg {
            reaction: Reaction::Focus,
            preserve_focus: false,
            ..Cfg::default()
        });
        h.frame();
        // The child is focused, the surface then takes the session…
        h.down(200.0, 400.0);
        h.down(40.0, 20.0);
        // …and the child is pressed again, which re-claims it.
        h.down(200.0, 400.0);
        h.clear_log();
        h.key();
        assert_eq!(
            h.log(),
            vec!["child:key focus=true".to_string()],
            "the surface's older link must not outrank the main tree's live one"
        );
    }

    #[test]
    fn a_surface_that_claims_nothing_never_disturbs_the_focused_child() {
        let mut h = Harness::new(Cfg::default());
        h.frame();
        h.down(200.0, 400.0);
        h.clear_log();
        h.down(40.0, 20.0);
        h.key();
        assert_eq!(
            h.log(),
            vec![
                "pod:Down@32,12".to_string(),
                "child:key focus=true".to_string()
            ],
            "an overlay press is not a blur"
        );
    }

    #[test]
    fn scrolling_an_ancestor_moves_the_surface_on_the_next_paint() {
        let mut h = Harness::new(Cfg {
            shape: Shape::Scrolled,
            placement: OverlayPlacement::default(),
            ..Cfg::default()
        });
        let before = rects(&h.frame());
        assert_eq!(
            before.last().copied(),
            Some(Rect::new(20.0, 244.0, 100.0, 274.0)),
            "placed under its anchor, 200px down the scrolled column"
        );
        h.root.event(
            &mut h.state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 10.0),
                delta: ScrollDelta::Pixels(0.0, 50.0),
            },
        );
        let after = rects(&h.frame());
        assert_eq!(
            after.last().copied(),
            Some(Rect::new(20.0, 194.0, 100.0, 224.0)),
            "the anchor moved with the scroll, and so did the surface"
        );
        // …and routing followed it: the pod's own rect is where the press lands.
        h.clear_log();
        h.down(30.0, 200.0);
        assert_eq!(h.log(), vec!["pod:Down@10,6".to_string()]);
    }

    #[test]
    fn clearing_the_overlay_removes_it_after_the_next_paint() {
        let mut h = Harness::new(Cfg::default());
        assert_eq!(rects(&h.frame()).last().copied(), Some(POD));
        h.state.cfg.open = false;
        let scene = h.frame();
        assert!(
            !rects(&scene).contains(&POD),
            "nothing floated is painted once the view is gone: {:?}",
            rects(&scene)
        );
        h.clear_log();
        h.down(40.0, 20.0);
        assert_eq!(
            h.log(),
            vec!["sib:Down@40,20".to_string(), "child:Down@40,20".to_string()],
            "and the press reaches the main tree again"
        );
    }

    // -----------------------------------------------------------------------
    // A slot over a `()`-typed pod: the framework-built-surface shape
    // -----------------------------------------------------------------------

    /// An owner with no children of its own that hosts one `()`-typed surface,
    /// builds the floated view itself (as a widget mounting a framework-built
    /// surface does) and drains whatever the surface dispatched.
    struct ToolbarHost {
        log: Log,
        claims_focus: bool,
    }

    struct ToolbarHostWidget {
        slot: OverlaySlot<()>,
        /// The mounted view, kept so the next rebuild has something to
        /// reconcile against.
        view: Option<AnyView<()>>,
        log: Log,
        claims_focus: bool,
    }

    impl ToolbarHost {
        fn pod(&self) -> AnyView<()> {
            any(UnitProbe {
                log: Rc::clone(&self.log),
                claims_focus: self.claims_focus,
            })
        }
    }

    impl View<()> for ToolbarHost {
        type Element = ToolbarHostWidget;
        fn build(&self, ctx: &mut BuildCtx<'_>) -> ToolbarHostWidget {
            let mut slot = OverlaySlot::new();
            slot.set_placement(corner());
            let view = self.pod();
            slot.rebuild(None, Some(&view), ctx);
            ToolbarHostWidget {
                slot,
                view: Some(view),
                log: Rc::clone(&self.log),
                claims_focus: self.claims_focus,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ToolbarHostWidget,
            ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.log = Rc::clone(&self.log);
            element.claims_focus = self.claims_focus;
            let view = self.pod();
            let flags = element
                .slot
                .rebuild(element.view.as_ref(), Some(&view), ctx);
            element.view = Some(view);
            flags
        }
    }

    impl Widget for ToolbarHostWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.slot.layout(ctx);
            bc.constrain(Size::new(100.0, 40.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            let size = ctx.size();
            self.slot.paint(ctx, size);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let mut pod_state = ();
            let Some(result) = self.slot.event(ctx, event, &mut pod_state) else {
                return EventResult::Ignored;
            };
            // Drained in the same pass the surface dispatched them in — the
            // queue is pass-scoped and is not a mailbox.
            for command in ctx.take_edit_commands() {
                self.log.borrow_mut().push(format!("cmd:{command:?}"));
            }
            result
        }
    }

    /// The `()`-typed floated content: it carries no application state at all,
    /// and speaks to its owner through the edit-command queue.
    struct UnitProbe {
        log: Log,
        claims_focus: bool,
    }

    struct UnitProbeWidget {
        log: Log,
        claims_focus: bool,
    }

    impl View<()> for UnitProbe {
        type Element = UnitProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> UnitProbeWidget {
            UnitProbeWidget {
                log: Rc::clone(&self.log),
                claims_focus: self.claims_focus,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut UnitProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.log = Rc::clone(&self.log);
            element.claims_focus = self.claims_focus;
            ChangeFlags::NONE
        }
    }

    impl Widget for UnitProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(80.0, 30.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                self.log.borrow_mut().push(format!(
                    "pod:{:?}@{},{}",
                    p.phase, p.position.x, p.position.y
                ));
                if p.phase == PointerPhase::Down {
                    if self.claims_focus {
                        ctx.request_focus();
                    }
                    ctx.dispatch_edit_command(EditCommand::Copy);
                    ctx.request_redraw();
                }
            }
            EventResult::Handled
        }
    }

    /// Drive the `()`-state host through a real root.
    fn unit_harness(claims_focus: bool) -> (RenderRoot<(), ToolbarHost>, Log) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut root: RenderRoot<(), ToolbarHost> = RenderRoot::new();
        let captured = Rc::clone(&log);
        let mut app_logic = move |_: &mut ()| ToolbarHost {
            log: Rc::clone(&captured),
            claims_focus,
        };
        let mut state = ();
        root.rebuild(&mut app_logic, &mut state);
        root.layout(WINDOW);
        root.paint(&mut RecordingScene::default(), FrameTime::from_nanos(0));
        (root, log)
    }

    #[test]
    fn a_unit_typed_pod_is_hosted_and_its_edit_commands_reach_the_owner() {
        let (mut root, log) = unit_harness(false);
        let mut state = ();
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(40.0, 50.0),
                button: PointerButton::Primary,
            }),
        );
        assert_eq!(
            log.borrow().clone(),
            vec!["pod:Down@32,10".to_string(), "cmd:Copy".to_string()],
            "the surface ran over its own `()` state and its command was drained"
        );
    }

    #[test]
    fn a_focus_claim_from_a_substituted_pod_still_opens_a_session() {
        let (mut root, _log) = unit_harness(true);
        let mut state = ();
        assert!(!root.is_focus_active());
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(40.0, 50.0),
                button: PointerButton::Primary,
            }),
        );
        assert!(
            root.is_focus_active(),
            "the substituted context's focus claim is mirrored onto the owner"
        );
    }

    #[test]
    fn a_slot_starts_closed_and_hands_its_outside_press_over_once() {
        let mut slot: OverlaySlot<()> = OverlaySlot::new();
        assert!(!slot.is_open());
        assert_eq!(slot.window_rect(), Rect::ZERO);
        assert!(!slot.take_outside_down());
        // Two slots never share an identity, which is the whole addressing rule.
        let other: OverlaySlot<()> = OverlaySlot::default();
        assert_ne!(slot.key(), other.key());
    }
}
