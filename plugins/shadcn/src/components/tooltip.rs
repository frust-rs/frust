//! `tooltip`: the hover-opened label that floats beside its trigger — and the
//! **shared hover latch** the hover card is built on too.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/tooltip.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `Tooltip` portalled to the body at `sideOffset={0}`, whose content is
//! `w-fit rounded-md bg-foreground px-3 py-1.5 text-xs text-background` entering
//! with `animate-in fade-in-0 zoom-in-95`, plus a `size-2.5 rotate-45
//! bg-foreground` arrow tip.
//!
//! # Riding the framework portal
//!
//! Every other anchored component in this catalog floats its panel through
//! [`crate::overlay::anchored`], which consumes every press outside its
//! content by design (its light dismiss) — mounted permanently under a hover
//! trigger it would eat the button press underneath. That mismatch used to
//! force this component to hand-roll its own full-area, permanently-mounted,
//! never-consumes-anything top layer, placed with its own copy of the
//! placement math.
//!
//! It no longer has to: the panel is registered through
//! [`frust::authoring::OverlaySlot`] — the same mechanism
//! [`frust::authoring::overlay_portal`] itself is built from, and the one the
//! framework's own docs name for exactly this shape ("a design system's
//! popover, menu, tooltip and context menu place a surface through `place`
//! and host it through `OverlaySlot`"). The root now paints the panel *after*
//! the whole main tree (so it can never be painted under a later sibling) and
//! hit-tests it *before* the main tree, per its declared
//! [`frust::OverlayBand`]/[`frust::OverlayInput`] — a tooltip declares
//! [`frust::OverlayBand::Tooltip`] + [`frust::OverlayInput::Transparent`], so
//! the root's pre-pass skips it outright and every press reaches the main
//! tree exactly as if the tooltip did not exist. A hover card's panel is
//! genuinely interactive instead ([`frust::OverlayBand::Floating`] +
//! [`frust::OverlayInput::Interactive`]), which the old hand-rolled layer
//! never allowed — its content (a link, a button) can now actually be
//! pressed.
//!
//! [`overlay_portal`](frust::authoring::overlay_portal)'s own declarative
//! wrapper anchors only to its own child's bounds, which does not fit here:
//! the trigger and the panel stay two independently-mounted views (so an app
//! keeps mounting `tooltip_trigger` and `tooltip` exactly as before), and the
//! panel's own [`OverlaySlot`](frust::authoring::OverlaySlot) anchors itself
//! to the trigger's captured rect via
//! [`OverlayAnchor::Rect`](frust::authoring::OverlayAnchor::Rect) — the shared
//! latch below is what carries that rect from one to the other.
//!
//! # Why this component is still a latch, not an anchored host
//!
//! A hover-opened overlay still cannot be driven the way a press-opened one
//! is, for the same two reasons as before:
//!
//! 1. **The delay has no event to fire on.** The open moment is "700ms after
//!    the pointer came to rest", and a resting pointer sends nothing. The
//!    only per-frame pass a widget gets is `paint`, which carries a clock
//!    ([`PaintCtx::frame_time`](frust::authoring::PaintCtx::frame_time)) but
//!    no application state, and the framework exposes no way for a
//!    plugin-tier widget to queue a state-bearing callback onto the next
//!    frame. So the open decision has to live in a widget, not in app state.
//! 2. **The trigger and the panel are two separately-mounted views.** An
//!    [`OverlaySlot`](frust::authoring::OverlaySlot) anchors to a rect *an
//!    app states explicitly* (`OverlayAnchor::Rect`), not to some other
//!    widget's bounds automatically — so the trigger's own window-space rect
//!    still has to travel from the widget that captures it to the widget
//!    that floats a surface against it.
//!
//! So the pair here is unchanged in shape: [`TooltipHover`], a shared,
//! non-reactive latch holding "is it open", the trigger's rect and whether
//! the pointer is over the panel (the same `Rc<Cell<_>>` shape
//! [`OverlayAnchor`](crate::overlay::OverlayAnchor) uses, and read the same
//! way — during the layout/paint of the very frame that wrote it);
//! [`tooltip_trigger`], which latches hover, runs the delays off the frame
//! clock, and writes the latch; and [`tooltip`], which now owns an
//! [`OverlaySlot`](frust::authoring::OverlaySlot) instead of a hand-rolled
//! layer.
//!
//! `on_open_change` is still reported, so an app can mirror the state — but
//! **best-effort and one pass late**: the widget can only call it from an
//! event pass, and the open it is reporting happened during a paint. It is a
//! notification, not the mechanism. Passing [`TooltipTriggerView::open`] a
//! `Some(_)` takes the whole decision over instead (Radix's controlled
//! `open`).
//!
//! # Delays
//!
//! [`TOOLTIP_DELAY_MS`] (700ms) is Radix's own `delayDuration` default;
//! shadcn's `TooltipProvider` overrides it to `0`, which
//! [`TooltipTriggerView::delay`] is one call away from. Closing is immediate
//! for a tooltip and delayed for a hover card ([`HOVER_CARD_CLOSE_DELAY_MS`]).
//! Neither delay is motion: `Theme.motion.reduce_motion` collapses the
//! fade/zoom entrance and leaves both timings alone.
//!
//! Radix's `skipDelayDuration` (a second tooltip inside the same provider
//! opens instantly) has no equivalent here: each trigger owns its own latch,
//! and there is no provider to share a skip window through.
//!
//! # The exit ramp
//!
//! `data-[state=closed]:fade-out-0` is a plain fade with no zoom riding
//! along. Its cost changed shape with the port: the *owner* widget
//! ([`TooltipLayerWidget`]) is what an app keeps mounted permanently — still,
//! as before, as a child of a full-area layered host such as
//! [`frust::Stack`], and no longer required to be its *topmost* one, since
//! paint order now comes from the declared [`frust::OverlayBand`] rather than
//! tree position — and it keeps *registering* the surface with the root for
//! as long as the fade needs, even after the latch has already dropped. The
//! fade/zoom itself runs inside the registered pod
//! ([`RampedPanelWidget`]) — the only widget that actually runs during the
//! root's separate overlay paint pass, which is why the ramp state lives
//! there and not on the owner: an owner's own `push_layer` would already
//! have been popped by the time the root gets around to painting a
//! registered pod directly.
//!
//! A closing panel consumes nothing, exactly as an open one does not — a
//! tooltip is input-transparent by contract — and its `over_panel` grip is
//! dropped with the latch, so a fading panel cannot hold its own card open.
//!
//! # Touch
//!
//! Touch has no hover, so a tooltip never opens on a touch device.
//! Long-press-opens-a-tooltip is not modelled.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, OutsideTap, OverlayAlign,
    OverlayAnchor, OverlayBand, OverlayInput, OverlayPlacement, OverlaySide, OverlaySlot, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, SemanticsCtx, Size, ThemeTextColor, ThemeTextType, View,
    Widget, any, build_child, erase_callback_arg, rebuild_child, route_event_single,
    teardown_child, visit_children,
};
use frust::{AnimationController, Curve, FrameTime, Theme, text};

use crate::hit::inside;
use crate::style;
use crate::tokens::ShadcnTokens;

/// Radix's `delayDuration` default: how long the pointer rests on a trigger
/// before its tooltip opens, in ms.
pub const TOOLTIP_DELAY_MS: u64 = 700;
/// Radix's `closeDelay` default for a hover card, in ms — the grace period that
/// lets the pointer travel from the trigger onto the panel.
pub const HOVER_CARD_CLOSE_DELAY_MS: u64 = 300;
/// `px-3` — the tooltip panel's horizontal padding.
const TOOLTIP_PAD_X: f64 = 12.0;
/// `py-1.5` — the tooltip panel's vertical padding.
const TOOLTIP_PAD_Y: f64 = 6.0;
/// `size-2.5` — the arrow tip's edge before its 45° rotation.
const ARROW_EDGE: f64 = 10.0;
/// `duration-200`, shared with every other overlay's entrance — and the exit
/// grace window [`TooltipLayerWidget`] keeps registering the panel for after
/// the latch drops, so the fade always gets the frames it needs.
const ENTRANCE_MS: u64 = 200;
/// `zoom-in-95` — the scale the entrance starts from.
const ZOOM_FROM: f64 = 0.95;
/// Below this the progress is treated as settled.
const PROGRESS_EPSILON: f64 = 1e-4;
/// Panel width used when the incoming constraints are horizontally unbounded.
const UNBOUNDED_WIDTH: f64 = 320.0;

/// The shared hover state of one tooltip (or hover card): whether it is open, the
/// trigger's window-space rect, and whether the pointer is over the panel.
///
/// Clone it — the clone shares the same cell. An app keeps one per tooltip,
/// hands a clone to the trigger and another to the layer.
///
/// **Not reactive**, exactly like [`OverlayAnchor`](crate::overlay::OverlayAnchor):
/// writing it wakes no frame. The trigger writes it during a paint that already
/// asked for the next frame, and the panel reads it during the layout and paint
/// of that frame.
#[derive(Clone, Debug, Default)]
pub struct TooltipHover(Rc<Cell<HoverLatch>>);

/// The latch's contents.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct HoverLatch {
    /// The trigger's rect, in window space.
    anchor: Rect,
    /// Whether the panel should be showing.
    open: bool,
    /// Whether the pointer is over the panel (which keeps it open).
    over_panel: bool,
}

impl TooltipHover {
    /// A fresh latch: closed, with no trigger rect captured yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the panel is showing.
    pub fn is_open(&self) -> bool {
        self.0.get().open
    }

    /// The trigger's captured rect, in window space.
    pub fn anchor(&self) -> Rect {
        self.0.get().anchor
    }

    /// Force the open state — the write [`TooltipTriggerView::open`] performs for
    /// a controlled tooltip, also usable by an app driving one itself.
    pub fn set_open(&self, open: bool) {
        let mut latch = self.0.get();
        latch.open = open;
        self.0.set(latch);
    }

    /// Record the trigger's rect.
    fn set_anchor(&self, anchor: Rect) {
        let mut latch = self.0.get();
        latch.anchor = anchor;
        self.0.set(latch);
    }

    /// Whether the pointer is over the panel.
    fn over_panel(&self) -> bool {
        self.0.get().over_panel
    }

    /// Record whether the pointer is over the panel.
    fn set_over_panel(&self, over: bool) {
        let mut latch = self.0.get();
        latch.over_panel = over;
        self.0.set(latch);
    }
}

/// Where the open/close timing has got to.
#[derive(Clone, Copy, Debug, PartialEq)]
enum HoverPhase {
    /// Closed, nothing pending.
    Idle,
    /// Hovered, waiting out the open delay from this frame time.
    Opening(FrameTime),
    /// Open.
    Open,
    /// Unhovered, waiting out the close delay from this frame time.
    Closing(FrameTime),
}

/// Whether a phase means the panel is on screen: open, or inside the close delay
/// that keeps it there while the pointer travels toward it.
fn showing(phase: HoverPhase) -> bool {
    matches!(phase, HoverPhase::Open | HoverPhase::Closing(_))
}

/// A view-held, typed open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// Wrap `child` as a hover trigger writing into `hover`.
///
/// Transparent in every other respect: it lays out, paints, routes events to and
/// publishes the semantics of `child` unchanged.
pub fn tooltip_trigger<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    child: V,
) -> TooltipTriggerView<State> {
    TooltipTriggerView {
        child: any(child),
        hover: hover.clone(),
        delay: Duration::from_millis(TOOLTIP_DELAY_MS),
        close_delay: Duration::ZERO,
        open: None,
        on_open_change: Rc::new(|_, _| {}),
    }
}

/// A declarative hover trigger. See [`tooltip_trigger`].
pub struct TooltipTriggerView<State: 'static> {
    child: AnyView<State>,
    hover: TooltipHover,
    delay: Duration,
    close_delay: Duration,
    open: Option<bool>,
    on_open_change: OnOpenChange<State>,
}

impl<State: 'static> TooltipTriggerView<State> {
    /// Set the open delay (Radix's `delayDuration`, [`TOOLTIP_DELAY_MS`] here).
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Set the close delay — how long the panel survives the pointer leaving both
    /// it and the trigger (Radix's `closeDelay`; zero for a tooltip).
    pub fn close_delay(mut self, close_delay: Duration) -> Self {
        self.close_delay = close_delay;
        self
    }

    /// Take the open state over: `Some(_)` pins the latch and switches the delay
    /// machine off, `None` (the default) leaves the widget in charge.
    pub fn open(mut self, open: Option<bool>) -> Self {
        self.open = open;
        self
    }

    /// Set the open-change callback — a best-effort notification, delivered on
    /// the first event pass after the transition (see the [module docs](self)).
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// The retained widget for a [`TooltipTriggerView`].
pub struct TooltipTriggerWidget {
    child: ChildPod,
    hover: TooltipHover,
    delay: Duration,
    close_delay: Duration,
    controlled: Option<bool>,
    phase: HoverPhase,
    /// The latched hover flag (self-corrected from `PaintCtx::is_hovered`).
    hovered: bool,
    /// A transition waiting for an event pass to report it.
    pending: Option<bool>,
    on_open_change: ErasedArgCallback<bool>,
}

impl TooltipTriggerWidget {
    /// Whether the trigger currently considers its overlay showing — open, or
    /// inside its close delay.
    pub fn is_open(&self) -> bool {
        showing(self.phase)
    }
}

impl<State: 'static> View<State> for TooltipTriggerView<State> {
    type Element = TooltipTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipTriggerWidget {
        if let Some(open) = self.open {
            self.hover.set_open(open);
        }
        TooltipTriggerWidget {
            child: build_child(&self.child, ctx),
            hover: self.hover.clone(),
            delay: self.delay,
            close_delay: self.close_delay,
            controlled: self.open,
            phase: if self.open == Some(true) {
                HoverPhase::Open
            } else {
                HoverPhase::Idle
            },
            hovered: false,
            pending: None,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        element.hover = self.hover.clone();
        element.delay = self.delay;
        element.close_delay = self.close_delay;
        if let Some(open) = self.open {
            // Controlled: the app's value wins every rebuild, and the delay
            // machine is parked on it.
            self.hover.set_open(open);
            element.phase = if open {
                HoverPhase::Open
            } else {
                HoverPhase::Idle
            };
        }
        element.controlled = self.open;
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut TooltipTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for TooltipTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative — it is what drops the flag on
        // the frame the pointer moves onto something else, which no event can
        // tell this widget about — and `PaintCtx::origin` is absolute window
        // space, which is what the panel's placement needs.
        self.hovered = ctx.is_hovered();
        self.hover
            .set_anchor(Rect::from_origin_size(ctx.origin(), ctx.size()));

        if self.controlled.is_none() {
            // Focus opens a tooltip too (Radix's `focus` trigger), and the panel
            // keeps it open while the pointer is over it.
            let active = self.hovered || ctx.has_focus() || self.hover.over_panel();
            let now = ctx.frame_time();
            let previous = self.phase;
            self.phase = match (self.phase, active) {
                (HoverPhase::Idle, true) => HoverPhase::Opening(now),
                (HoverPhase::Opening(since), true) => {
                    if now.saturating_sub(since) >= self.delay {
                        HoverPhase::Open
                    } else {
                        HoverPhase::Opening(since)
                    }
                }
                (HoverPhase::Opening(_), false) => HoverPhase::Idle,
                (HoverPhase::Open, false) => {
                    if self.close_delay.is_zero() {
                        HoverPhase::Idle
                    } else {
                        HoverPhase::Closing(now)
                    }
                }
                (HoverPhase::Closing(since), false) => {
                    if now.saturating_sub(since) >= self.close_delay {
                        HoverPhase::Idle
                    } else {
                        HoverPhase::Closing(since)
                    }
                }
                (HoverPhase::Closing(_), true) => HoverPhase::Open,
                (phase, _) => phase,
            };
            // A running delay needs the next frame to measure against; nothing
            // else in the tree is animating while a pointer rests.
            if matches!(self.phase, HoverPhase::Opening(_) | HoverPhase::Closing(_)) {
                ctx.request_frame();
            }
            // A card inside its close delay is still showing — the grace period
            // is what lets the pointer cross onto the panel.
            let open = showing(self.phase);
            if open != showing(previous) {
                self.hover.set_open(open);
                if !open {
                    self.hover.set_over_panel(false);
                }
                self.pending = Some(open);
                ctx.request_frame();
            }
        }

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Any pass will do to flush the notification the paint could not make —
        // including the broadcast, which is why this runs before the routing.
        if let Some(open) = self.pending.take() {
            (self.on_open_change)(ctx, open);
        }
        let routed = route_event_single(&mut self.child, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        let InputEvent::Pointer(p) = event else {
            return routed;
        };
        if p.phase == PointerPhase::Move {
            // Claimed after the routing above (the claim-ordering rule): a
            // hovered control inside the trigger wins the claim and this wrapper
            // still reads hovered through the path.
            let over = inside(p.position, ctx.size());
            if over {
                ctx.claim_hover();
            }
            if self.hovered != over {
                self.hovered = over;
                ctx.request_redraw();
            }
        }
        routed
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

/// The chrome a [`RampedPanelWidget`] paints around its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TooltipArrow {
    /// The tooltip's `size-2.5 rotate-45 bg-foreground` tip.
    Tip,
    /// No tip (the hover card has none).
    None,
}

/// Build a tooltip panel showing `label` while `hover` is open.
///
/// Mount it *unconditionally*, as a child of the same full-area layered host
/// as before (e.g. [`frust::Stack`]) — it registers nothing with the root
/// while closed, so it costs a layout of its content and nothing else. Unlike
/// the layer this replaced, it no longer has to be that host's *topmost*
/// child: the root's own [`frust::OverlayBand`] ordering puts it above
/// everything regardless of where among the host's children it sits.
pub fn tooltip<State: 'static>(
    hover: &TooltipHover,
    label: impl Into<String>,
) -> TooltipLayerView<State> {
    let content: AnyView<State> = any(TooltipPanel {
        label: label.into(),
    });
    TooltipLayerView {
        content: any(RampedPanelView {
            content,
            hover: hover.clone(),
            arrow: TooltipArrow::Tip,
        }),
        hover: hover.clone(),
        // `sideOffset={0}` — the tooltip sits flush against its trigger, with the
        // arrow tip bridging the gap.
        placement: OverlayPlacement::on(OverlaySide::Top).offset(0.0),
        band: OverlayBand::Tooltip,
        input: OverlayInput::Transparent,
        outside_tap: OutsideTap::Ignore,
    }
}

/// Build a panel showing `content` while `hover` is open — the hover card's half
/// of the shared machinery. See [`tooltip`].
///
/// Unlike a tooltip, `content` is genuinely interactive: it is registered
/// [`frust::OverlayInput::Interactive`], so a link or button inside it can
/// actually be pressed.
pub(crate) fn tooltip_layer<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    content: V,
) -> TooltipLayerView<State> {
    let content: AnyView<State> = any(content);
    TooltipLayerView {
        content: any(RampedPanelView {
            content,
            hover: hover.clone(),
            arrow: TooltipArrow::None,
        }),
        hover: hover.clone(),
        placement: OverlayPlacement::default(),
        band: OverlayBand::Floating,
        input: OverlayInput::Interactive,
        // Notified but not consumed: a hover card closes on hover-out, never on
        // a tap outside it (see the module docs on why it stays a latch).
        outside_tap: OutsideTap::Notify { consume: false },
    }
}

/// A declarative hover-overlay panel. See [`tooltip`].
pub struct TooltipLayerView<State: 'static> {
    /// Wraps a [`RampedPanelView`] — already erased here so [`tooltip`] and
    /// [`tooltip_layer`] share one shape regardless of what they float.
    content: AnyView<State>,
    hover: TooltipHover,
    placement: OverlayPlacement,
    band: OverlayBand,
    input: OverlayInput,
    outside_tap: OutsideTap,
}

impl<State: 'static> TooltipLayerView<State> {
    /// Set the side the panel opens on.
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self
    }

    /// Set the cross-axis alignment.
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self
    }

    /// Set the gap between trigger and panel.
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self
    }
}

/// The retained widget for a [`TooltipLayerView`]: owns an
/// [`OverlaySlot`], anchored to the trigger's rect captured in `hover`, and
/// decides only *whether* to keep registering it — the ramp itself runs in
/// the registered pod ([`RampedPanelWidget`]), which is the only widget that
/// runs during the root's own overlay paint pass.
pub struct TooltipLayerWidget<State: 'static> {
    hover: TooltipHover,
    slot: OverlaySlot<State>,
    /// Whether the latch was open as of the last paint.
    was_open: bool,
    /// When the latch last dropped, if a fade might still be running.
    closing_since: Option<FrameTime>,
}

impl<State: 'static> TooltipLayerView<State> {
    /// Push the view's routing configuration onto the slot — the half of
    /// `build`/`rebuild` that is identical in both.
    fn configure(&self, slot: &mut OverlaySlot<State>) {
        slot.set_band(self.band);
        slot.set_input(self.input);
        slot.set_outside_tap(self.outside_tap);
        slot.set_placement(self.placement);
    }
}

impl<State: 'static> View<State> for TooltipLayerView<State> {
    type Element = TooltipLayerWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipLayerWidget<State> {
        let mut slot = OverlaySlot::new();
        self.configure(&mut slot);
        slot.rebuild(None, Some(&self.content), ctx);
        TooltipLayerWidget {
            hover: self.hover.clone(),
            slot,
            was_open: false,
            closing_since: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipLayerWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.configure(&mut element.slot);
        element.hover = self.hover.clone();
        element
            .slot
            .rebuild(Some(&prev.content), Some(&self.content), ctx)
            | ChangeFlags::PAINT
    }

    fn teardown(&self, element: &mut TooltipLayerWidget<State>, ctx: &mut BuildCtx<'_>) {
        element.slot.rebuild(Some(&self.content), None, ctx);
    }
}

impl<State: 'static> Widget for TooltipLayerWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The floated pod is sized against the window (`OverlaySlot::layout`),
        // not against `bc` — it escapes this widget's box entirely, and unlike
        // the layer this replaced, that no longer depends on `bc` being bounded
        // (the old scroll-view trap this component used to hit): the pod is
        // never coerced from an unbounded `bc.max()`, because it never reads
        // `bc.max()` at all.
        self.slot.layout(ctx);
        // Filling the available box, not `Size::ZERO`: an ordinary,
        // uncaptured, non-broadcast pointer move only ever reaches a widget
        // whose own bounds contain it (`route_event`'s hit test), and this
        // owner's own `event` needs exactly that to learn "the pointer moved
        // off the panel" (the registry has no companion event for it — see
        // `event` below). Coerced to zero on an unbounded axis rather than
        // reporting an infinite size upward.
        let max = bc.max();
        bc.constrain(Size::new(
            if max.width.is_finite() {
                max.width
            } else {
                0.0
            },
            if max.height.is_finite() {
                max.height
            } else {
                0.0
            },
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        // Whether to keep registering the surface at all — the fade the
        // registered pod runs needs frames *after* the latch has already
        // dropped, matching the old layer's "still fading" edge case, so this
        // mirrors the same `ENTRANCE_MS` duration the pod ramps over rather
        // than trusting the latch alone.
        let now = ctx.frame_time();
        let open = self.hover.is_open();
        if open {
            self.was_open = true;
            self.closing_since = None;
        } else if self.was_open {
            self.was_open = false;
            self.closing_since = Some(now);
        }
        let reduce_motion = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let grace = if reduce_motion {
            Duration::ZERO
        } else {
            Duration::from_millis(ENTRANCE_MS)
        };
        let showing =
            open || matches!(self.closing_since, Some(since) if now.saturating_sub(since) < grace);
        if !showing {
            return;
        }
        // Window space → this widget's own local space: the same translation
        // `place`'s every other caller in this workspace makes, now handed to
        // the slot instead of computed by hand.
        let local = self.hover.anchor() - ctx.origin().to_vec2();
        self.slot.set_anchor(OverlayAnchor::Rect(local));
        self.slot.paint(ctx, Size::ZERO);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(result) = self.slot.event_ambient(ctx, event) {
            // Drained, not acted on: a hover card closes on hover-out (the
            // trigger's own close delay), never on a tap outside it.
            self.slot.take_outside_down();
            return result;
        }
        // Reaching here means the root's overlay pre-pass did *not* claim this
        // event for our own registered rect (no `InputEvent::Overlay` for our
        // key, no active capture, no focus routed to the pod) — the only way
        // this owner, a regular main-tree widget, sees a pointer move at all
        // while the panel is interactive. The registry has no "you left"
        // companion event for `OverlayInput::Interactive`, so an *ordinary*
        // move landing here is itself the signal that the pointer is not over
        // the panel — the panel's own pod ([`RampedPanelWidget`]) is what sets
        // the flag back to `true` from inside the routed overlay event.
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Move
            && self.hover.over_panel()
        {
            self.hover.set_over_panel(false);
            ctx.request_redraw();
        }
        EventResult::Ignored
    }

    fn semantics(&self, _ctx: &mut SemanticsCtx) {
        // An overlay pod contributes no accessibility nodes in v1 (see
        // `frust_widgets::overlay`'s "Not in v1" list) — nothing to attach.
    }

    visit_children!();
}

/// The pod actually registered with the root's overlay mechanism: `content`
/// wrapped in the shared latch's own fade/zoom ramp, plus (for a tooltip) its
/// arrow tip.
///
/// This is the *only* widget that runs during the root's separate overlay
/// paint pass, which is why the ramp state lives here rather than on
/// [`TooltipLayerWidget`]: the root paints a registered pod directly, so an
/// owner's own `push_layer`/`push_transform` would already have been popped
/// by the time the pod's own paint runs.
struct RampedPanelView<State: 'static> {
    content: AnyView<State>,
    hover: TooltipHover,
    arrow: TooltipArrow,
}

/// The retained widget for a [`RampedPanelView`].
struct RampedPanelWidget {
    content: ChildPod,
    hover: TooltipHover,
    arrow: TooltipArrow,
    /// Whether the panel was showing on the last paint (an edge starts a ramp,
    /// in whichever direction the edge went).
    was_open: bool,
    anim: AnimationController,
    /// How present the panel is: `0.0` gone, `1.0` settled.
    presence: f64,
}

impl RampedPanelWidget {
    /// Take this paint's ramp decision, returning the presence to paint at.
    ///
    /// The entrance is `fade-in-0 zoom-in-95`; the exit is
    /// `data-[state=closed]:fade-out-0` — a plain fade, with no zoom riding
    /// along, which is why the caller consults `open` before pushing the
    /// transform.
    fn ramp(&mut self, ctx: &mut PaintCtx, open: bool, reduce_motion: bool) -> f64 {
        if open != self.was_open {
            self.was_open = open;
            if reduce_motion {
                self.anim.stop();
                self.presence = if open { 1.0 } else { 0.0 };
            } else if open {
                // Both start from the controller's current value, so a hover
                // returning mid-fade-out picks the entrance up from there.
                self.anim.forward();
            } else {
                self.anim.reverse();
            }
        }
        if reduce_motion {
            // `reduce_motion` collapses either direction to a jump. The open and
            // close delays are timing, not motion, and are untouched (they live
            // on the trigger).
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.presence = if open { 1.0 } else { 0.0 };
        } else if self.anim.is_animating() {
            if self.anim.advance(ctx.frame_time()) {
                ctx.request_frame();
            }
            let next = self.anim.value_clamped();
            if (next - self.presence).abs() > PROGRESS_EPSILON {
                // One more frame to paint the value just computed — including
                // the settled one the final advance lands on.
                ctx.request_frame();
            }
            self.presence = next;
        }
        self.presence
    }
}

impl<State: 'static> View<State> for RampedPanelView<State> {
    type Element = RampedPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RampedPanelWidget {
        RampedPanelWidget {
            content: build_child(&self.content, ctx),
            hover: self.hover.clone(),
            arrow: self.arrow,
            was_open: false,
            anim: AnimationController::new(Duration::from_millis(ENTRANCE_MS))
                .with_curve(Curve::EaseOut),
            presence: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RampedPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        element.hover = self.hover.clone();
        element.arrow = self.arrow;
        flags
    }

    fn teardown(&self, element: &mut RampedPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for RampedPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let content = self.content.layout_child(ctx, bc);
        self.content.set_origin(Point::ORIGIN);
        bc.constrain(content)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let open = self.hover.is_open();
        // Settled closed: paint nothing. `was_open` is part of the test because
        // a latch that closes on the frame after it opened leaves the presence
        // at zero with the edge still owed — skipping the ramp there would
        // strand `was_open` set and the next open would never start.
        if !open && !self.was_open && self.presence <= 0.0 {
            return;
        }
        let reduce_motion = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let presence = self.ramp(ctx, open, reduce_motion);
        if !open && presence <= 0.0 {
            // The fade-out has just settled.
            return;
        }

        let origin = ctx.origin();
        let size = ctx.size();
        let panel = Rect::from_origin_size(origin, size);
        let ramping = presence < 1.0;
        if ramping {
            scene.push_layer(panel.origin(), panel.size(), presence as f32);
            if open {
                // `zoom-in-95` rides the entrance only — upstream's exit is
                // `fade-out-0` with no zoom of its own.
                let c = panel.center();
                let scale = ZOOM_FROM + (1.0 - ZOOM_FROM) * presence;
                scene.push_transform(
                    Affine::translate(c.to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-c.to_vec2()),
                );
            }
        }
        if self.arrow == TooltipArrow::Tip {
            // Both rects in window space: `panel` is this layer's placed rect,
            // built from `ctx.origin()` above, and the latch stores the
            // trigger's rect in window space already. Lowering the anchor into
            // the layer's own space instead would compare the two in different
            // spaces and put the tip `origin` away from the edge facing its
            // trigger — an identity only for a layer sitting at the window
            // origin, which a floated overlay is not.
            let fill = crate::overlay::foreground(Theme::from_paint_ctx(ctx));
            draw_arrow(scene, panel, self.hover.anchor(), fill);
        }
        self.content.paint_child(ctx, scene);
        if ramping {
            if open {
                scene.pop_transform();
            }
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Keep the latch's "pointer is over the panel" flag current — a
        // tooltip is `OverlayInput::Transparent`, so the root never routes an
        // overlay event here at all and this never runs for one; a hover
        // card's panel is `OverlayInput::Interactive`, so its own content
        // (a link, a button) is routed to below, exactly like any other
        // container.
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Move
        {
            let over = inside(p.position, ctx.size());
            if over != self.hover.over_panel() {
                self.hover.set_over_panel(over);
                ctx.request_redraw();
            }
        }
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.hover.is_open() {
            self.content.semantics_child(ctx);
        }
    }

    visit_children!(content);
}

/// Keep `v` inside `[lo, hi]` by at least `reach`, the inset a tip of
/// half-diagonal `reach` needs to sit wholly within that span.
///
/// **Total for a span narrower than the tip**, which [`f64::clamp`] is not: at
/// `hi - lo <= 2 * reach` the two bounds cross (`lo + reach > hi - reach`) and
/// `clamp` panics on `min > max` rather than answering. No position keeps the
/// tip inside a span that short, so the midpoint — as far inside as it can be —
/// is the answer. See [`draw_arrow`] for how short a panel gets.
fn inset_within(v: f64, lo: f64, hi: f64, reach: f64) -> f64 {
    if hi - lo <= 2.0 * reach {
        (lo + hi) / 2.0
    } else {
        v.clamp(lo + reach, hi - reach)
    }
}

/// Paint the tooltip's `size-2.5 rotate-45` tip on the panel edge facing the
/// anchor.
///
/// `panel` and `anchor` are both **window space** — the space the scene is
/// recorded in and the space the latch stores its trigger rect in. Passing one
/// of them in the layer's own local space instead puts the tip the layer's
/// origin away from the edge it belongs on.
///
/// Defined for a panel of any extent, including none: a viewport too small to
/// give the panel a size collapses it (`crate::overlay`'s `finite_or_zero`
/// coerces an unbounded constraint to zero, and a host can lay a frame out at a
/// viewport it has not measured yet), and a tip is geometry hung off a panel
/// edge — with no panel there is no edge, and a shorter one takes the tip as
/// far inside as it goes.
fn draw_arrow(scene: &mut dyn PaintScene, panel: Rect, anchor: Rect, color: Color) {
    if panel.is_zero_area() {
        return;
    }
    // A square rotated 45° is a diamond whose half-diagonal is `edge / √2`.
    let reach = ARROW_EDGE * std::f64::consts::FRAC_1_SQRT_2;
    let (center, horizontal) = if panel.y1 <= anchor.y0 {
        (Point::new(anchor.center().x, panel.y1), false)
    } else if panel.y0 >= anchor.y1 {
        (Point::new(anchor.center().x, panel.y0), false)
    } else if panel.x1 <= anchor.x0 {
        (Point::new(panel.x1, anchor.center().y), true)
    } else {
        (Point::new(panel.x0, anchor.center().y), true)
    };
    // Keep the tip inside the panel's own span, so a clamped panel does not grow
    // a tip hanging off its corner.
    let center = if horizontal {
        Point::new(center.x, inset_within(center.y, panel.y0, panel.y1, reach))
    } else {
        Point::new(inset_within(center.x, panel.x0, panel.x1, reach), center.y)
    };
    let mut path = frust::authoring::BezPath::new();
    path.move_to(Point::new(-reach, 0.0));
    path.line_to(Point::new(0.0, -reach));
    path.line_to(Point::new(reach, 0.0));
    path.line_to(Point::new(0.0, reach));
    path.close_path();
    scene.fill_path(center, &path, &Brush::Solid(color));
}

/// The tooltip's own panel: `w-fit rounded-md bg-foreground px-3 py-1.5 text-xs
/// text-background`.
///
/// The ink is [`ThemeTextColor::OnPrimary`] rather than a `--background` role,
/// which the authoring seam's themed text colors do not carry. The two are the
/// same near-white/near-black inversion in every shipped preset — shadcn's
/// `--primary-foreground` is exactly the ink meant to sit on the dark neutral
/// this panel is filled with, and it flips with brightness the way an explicit
/// color could not.
struct TooltipPanel {
    label: String,
}

/// The retained widget for a [`TooltipPanel`].
struct TooltipPanelWidget {
    label: ChildPod,
}

impl TooltipPanel {
    /// The label child: `text-xs` in the live theme's `BodySmall` family.
    // erasure: keep built into a ChildPod: build_child/rebuild_child/teardown_child take &AnyView
    fn label_view<State: 'static>(&self) -> AnyView<State> {
        any(text(self.label.clone())
            .size(style::TEXT_XS as f32)
            .themed_family(ThemeTextType::BodySmall)
            .themed_role(ThemeTextColor::OnPrimary))
    }
}

impl<State: 'static> View<State> for TooltipPanel {
    type Element = TooltipPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipPanelWidget {
        TooltipPanelWidget {
            label: build_child(&self.label_view::<State>(), ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(
            &prev.label_view::<State>(),
            &self.label_view::<State>(),
            &mut element.label,
            ctx,
        )
    }

    fn teardown(&self, element: &mut TooltipPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.label_view::<State>(), &mut element.label, ctx);
    }
}

impl Widget for TooltipPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };
        let inner = (available - 2.0 * TOOLTIP_PAD_X).max(0.0);
        let label = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(Size::new(inner, f64::INFINITY)));
        self.label
            .set_origin(Point::new(TOOLTIP_PAD_X, TOOLTIP_PAD_Y));
        // `w-fit`: the panel shrink-wraps its label.
        bc.constrain(Size::new(
            label.width + 2.0 * TOOLTIP_PAD_X,
            label.height + 2.0 * TOOLTIP_PAD_Y,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        let (fill, radius) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                crate::overlay::foreground(theme),
                ShadcnTokens::resolve_radius(None, theme).md,
            )
        };
        scene.fill_rounded_rect(origin, size, radius, fill);
        self.label.paint_child(ctx, scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.label.semantics_child(ctx);
    }

    visit_children!(label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, ft_ms, light, pointer};
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        opens: Vec<bool>,
        button_pressed: bool,
    }

    const TRIGGER: Size = Size::new(80.0, 36.0);

    /// A page widget filling whatever area it is given, recording whether it
    /// was pressed — the proof consumer for "a press on the button beneath an
    /// open tooltip actually reaches the button", not merely "nobody handled
    /// it".
    struct PageButton;

    struct PageButtonWidget;

    impl View<AppState> for PageButton {
        type Element = PageButtonWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PageButtonWidget {
            PageButtonWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PageButtonWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
        fn teardown(&self, _element: &mut PageButtonWidget, _ctx: &mut BuildCtx<'_>) {}
    }

    impl Widget for PageButtonWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.state_mut::<AppState>().button_pressed = true;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
        fn semantics(&self, _ctx: &mut SemanticsCtx) {}
        visit_children!();
    }

    /// The mount an app uses: the trigger in the page, the panel mounted
    /// unconditionally alongside it — the panel no longer needs to be a
    /// particular child of anything (see the module docs).
    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        hover: TooltipHover,
        controlled: Option<bool>,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                hover: TooltipHover::new(),
                controlled: None,
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        /// One whole frame at `ms`: rebuild, layout, paint.
        fn frame(&mut self, ms: f64) {
            let hover = self.hover.clone();
            let controlled = self.controlled;
            let mut logic = move |_s: &mut AppState| {
                frust::stack()
                    .child(
                        tooltip_trigger(
                            &hover,
                            SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                        )
                        .open(controlled)
                        .on_open_change(|s: &mut AppState, open| s.opens.push(open)),
                    )
                    .child(tooltip(&hover, "Add to library"))
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            self.root.paint(&mut Recorder::default(), ft_ms(ms));
        }

        fn paint_at(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }
    }

    #[test]
    fn the_panel_opens_only_after_the_delay_and_closes_the_moment_hover_ends() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        assert!(!h.hover.is_open(), "the delay has not elapsed");

        h.frame(TOOLTIP_DELAY_MS as f64 - 1.0);
        assert!(!h.hover.is_open(), "still short of it");

        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open(), "700ms of rest opens it");

        // The pointer leaves: the authoritative paint-time hover read closes it
        // with no close delay of its own.
        h.event(pointer(PointerPhase::Move, 300.0, 300.0));
        h.frame(TOOLTIP_DELAY_MS as f64 + 16.0);
        assert!(!h.hover.is_open());
    }

    #[test]
    fn the_open_transition_is_reported_on_the_next_event_pass() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert_eq!(h.state.opens, Vec::<bool>::new(), "paint cannot report");
        h.event(pointer(PointerPhase::Move, 12.0, 10.0));
        assert_eq!(h.state.opens, vec![true], "the next pass flushes it");
    }

    #[test]
    fn a_controlled_open_pins_the_latch_and_bypasses_the_delay() {
        let mut h = Harness::new();
        h.controlled = Some(true);
        h.frame(0.0);
        assert!(h.hover.is_open(), "no hover, no delay, still open");
        h.controlled = Some(false);
        h.frame(16.0);
        assert!(!h.hover.is_open());
        // Hovering does not move a controlled tooltip.
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(1000.0);
        assert!(!h.hover.is_open());
    }

    #[test]
    fn the_panel_paints_nothing_while_closed_and_a_panel_with_a_tip_while_open() {
        let mut h = Harness::new();
        let closed = h.paint_at(0.0);
        assert!(closed.rrects.is_empty(), "no panel while closed");

        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        // Past the entrance ramp, so the panel composites plainly.
        let open = h.paint_at(TOOLTIP_DELAY_MS as f64 + ENTRANCE_MS as f64 * 2.0);
        let theme = light();
        assert!(
            open.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().on_surface),
            "bg-foreground panel"
        );
        assert!(
            open.rects
                .iter()
                .any(|(_, s, c)| *c == theme.scheme().on_surface && s.width > 0.0),
            "the rotated tip, filled in the same token"
        );
    }

    #[test]
    fn the_entrance_fades_and_zooms_and_reduce_motion_jumps() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        let start = TOOLTIP_DELAY_MS as f64;
        let first = h.paint_at(start);
        assert_eq!(first.layers, vec![0.0], "starts transparent");
        let mid = h.paint_at(start + ENTRANCE_MS as f64 / 2.0);
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);
        assert_eq!(mid.transforms.len(), 1, "and a zoom");
        let done = h.paint_at(start + ENTRANCE_MS as f64 * 2.0);
        assert!(done.layers.is_empty() && done.transforms.is_empty());

        // Reduced motion: open on the same schedule, with no ramp at all.
        let mut reduced = light();
        reduced.motion.reduce_motion = true;
        let mut h = Harness::new();
        h.root.set_theme(Box::new(reduced));
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        let jumped = h.paint_at(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open(), "the delay is timing, not motion");
        assert!(jumped.layers.is_empty(), "and the entrance is a jump");
    }

    impl Harness {
        /// Open the tooltip and settle its entrance, returning the frame time
        /// the panel is fully present at.
        fn opened(&mut self) -> f64 {
            self.event(pointer(PointerPhase::Move, 10.0, 10.0));
            self.frame(0.0);
            self.frame(TOOLTIP_DELAY_MS as f64);
            let settled = TOOLTIP_DELAY_MS as f64 + ENTRANCE_MS as f64 * 2.0;
            self.paint_at(settled);
            settled
        }

        /// Move the pointer off the trigger and run the frame that closes the
        /// latch, returning that frame's time.
        fn unhovered(&mut self, from: f64) -> f64 {
            self.event(pointer(PointerPhase::Move, 300.0, 300.0));
            let closed = from + 16.0;
            self.frame(closed);
            closed
        }
    }

    #[test]
    fn the_panel_fades_out_when_the_hover_ends_and_then_stops_painting() {
        let mut h = Harness::new();
        let settled = h.opened();
        assert!(h.hover.is_open());
        let closed = h.unhovered(settled);
        assert!(!h.hover.is_open(), "the latch drops on the same frame");

        let mid = h.paint_at(closed + ENTRANCE_MS as f64 / 2.0);
        assert!(!mid.rrects.is_empty(), "the panel is still on screen");
        assert!(
            mid.layers[0] > 0.0 && mid.layers[0] < 1.0,
            "at a partial alpha"
        );
        assert!(
            mid.transforms.is_empty(),
            "a plain fade-out — the zoom rides the entrance only"
        );
        let later = h.paint_at(closed + ENTRANCE_MS as f64 * 0.75);
        assert!(later.layers[0] < mid.layers[0], "presence is decreasing");

        let gone = h.paint_at(closed + ENTRANCE_MS as f64 * 2.0);
        assert!(gone.rrects.is_empty(), "nothing painted at settle");
        // And it stays gone.
        assert!(
            h.paint_at(closed + ENTRANCE_MS as f64 * 4.0)
                .rrects
                .is_empty()
        );
    }

    #[test]
    fn re_hovering_mid_fade_out_picks_the_entrance_back_up_from_there() {
        let mut h = Harness::new();
        let settled = h.opened();
        let closed = h.unhovered(settled);
        let mid = h.paint_at(closed + ENTRANCE_MS as f64 / 2.0);
        let interrupted = mid.layers[0];

        // Back on the trigger: the latch re-opens (the trigger is past its own
        // delay while the pointer is on it again from `Closing`/`Idle`).
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(closed + ENTRANCE_MS as f64 / 2.0);
        h.frame(closed + ENTRANCE_MS as f64 / 2.0 + TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());
        let resumed = h.paint_at(closed + ENTRANCE_MS as f64 / 2.0 + TOOLTIP_DELAY_MS as f64);
        assert_eq!(
            resumed.layers[0], interrupted,
            "the entrance resumes at the presence the fade-out reached"
        );
    }

    #[test]
    fn a_latch_that_closes_on_the_frame_after_it_opened_still_reopens() {
        // The narrow window the ramp has to survive: the panel opened but never
        // got a frame to ramp in, so it is closing from a presence of zero.
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());
        h.event(pointer(PointerPhase::Move, 300.0, 300.0));
        h.frame(TOOLTIP_DELAY_MS as f64 + 16.0);
        assert!(!h.hover.is_open());

        // Back on the trigger: the entrance must run properly this time.
        let base = TOOLTIP_DELAY_MS as f64 + 16.0;
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(base);
        h.frame(base + TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());
        let settled = h.paint_at(base + TOOLTIP_DELAY_MS as f64 + ENTRANCE_MS as f64 * 2.0);
        assert!(
            !settled.rrects.is_empty(),
            "the panel is on screen again, fully composited"
        );
        assert!(settled.layers.is_empty(), "the entrance ran to completion");
    }

    #[test]
    fn reduce_motion_takes_the_panel_away_on_the_frame_the_hover_ends() {
        let mut reduced = light();
        reduced.motion.reduce_motion = true;
        let mut h = Harness::new();
        h.root.set_theme(Box::new(reduced));
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(!h.paint_at(TOOLTIP_DELAY_MS as f64).rrects.is_empty());

        h.event(pointer(PointerPhase::Move, 300.0, 300.0));
        let closed = TOOLTIP_DELAY_MS as f64 + 16.0;
        h.frame(closed);
        assert!(
            h.paint_at(closed).rrects.is_empty(),
            "no fade-out at all — gone the frame the latch drops"
        );
    }

    #[test]
    fn the_panel_consumes_nothing_so_the_page_under_it_keeps_its_input() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());
        let panel = {
            let rect = frust::authoring::place(
                h.hover.anchor(),
                Size::new(100.0, 28.0),
                Rect::from_origin_size(Point::ORIGIN, WINDOW),
                OverlayPlacement::on(OverlaySide::Top).offset(0.0),
            );
            rect.center()
        };
        let outcome = h
            .root
            .event(&mut h.state, &pointer(PointerPhase::Down, panel.x, panel.y));
        assert!(
            !outcome.handled,
            "a press over the panel is never swallowed"
        );
    }

    /// The proof this port owes: a press on a real widget beneath an open
    /// tooltip must reach that widget, not just "nobody swallowed it". A page
    /// button fills the whole window, directly under wherever the tooltip
    /// panel lands.
    ///
    /// Negative control performed by hand: temporarily changing `tooltip`'s
    /// registration from `OverlayInput::Transparent` to
    /// `OverlayInput::Interactive` makes this test fail (`outcome.handled` is
    /// `false` and `state.button_pressed` stays `false`, because the root's
    /// overlay pre-pass then routes the press to the tooltip's own pod
    /// instead of letting it fall through).
    #[test]
    fn a_press_on_a_button_beneath_an_open_tooltip_reaches_the_button() {
        let hover = TooltipHover::new();
        let mut state = AppState::default();
        let mut root: RenderRoot<AppState, frust::StackView<AppState>> = RenderRoot::new();
        root.set_theme(Box::new(light()));
        let mut tcx = TextContext::new();

        fn logic(hover: &TooltipHover) -> impl FnMut(&mut AppState) -> frust::StackView<AppState> {
            let hover = hover.clone();
            move |_s: &mut AppState| {
                frust::stack()
                    .child(PageButton)
                    .child(tooltip_trigger(
                        &hover,
                        SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                    ))
                    .child(tooltip(&hover, "Add to library"))
            }
        }

        let mut run = |root: &mut RenderRoot<AppState, frust::StackView<AppState>>,
                       state: &mut AppState,
                       ms: f64| {
            let mut l = logic(&hover);
            root.rebuild(&mut l, state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft_ms(ms));
        };

        run(&mut root, &mut state, 0.0);
        root.event(&mut state, &pointer(PointerPhase::Move, 10.0, 10.0));
        run(&mut root, &mut state, 0.0);
        run(&mut root, &mut state, TOOLTIP_DELAY_MS as f64);

        assert!(hover.is_open(), "the tooltip is open");

        let panel_center = frust::authoring::place(
            hover.anchor(),
            Size::new(100.0, 28.0),
            Rect::from_origin_size(Point::ORIGIN, WINDOW),
            OverlayPlacement::on(OverlaySide::Top).offset(0.0),
        )
        .center();
        let outcome = root.event(
            &mut state,
            &pointer(PointerPhase::Down, panel_center.x, panel_center.y),
        );
        assert!(
            outcome.handled,
            "the press reaches the page button under the open tooltip"
        );
        assert!(
            state.button_pressed,
            "and the button beneath actually fired, not just 'nobody swallowed it'"
        );
    }

    #[test]
    fn a_viewport_too_small_to_size_the_panel_leaves_the_tip_off_instead_of_panicking() {
        // A host may lay a frame out before it knows its own viewport — winit's
        // web backend reports `inner_size()` as 0x0 until its `ResizeObserver`
        // first fires, and the browser shell lays out at exactly that (see
        // `frust-shell-web`'s surface reconciliation, which guards the
        // swapchain against a zero dimension but not the layout pass). The
        // panel collapses with the area, and a tip whose half-diagonal exceeds
        // the panel it hangs off used to take `f64::clamp` past `min > max` and
        // panic out through the event loop.
        let hover = TooltipHover::new();
        hover.set_open(true);
        let mut root: RenderRoot<AppState, frust::StackView<AppState>> = RenderRoot::new();
        root.set_theme(Box::new(light()));
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let latch = hover.clone();
        let mut logic = move |_s: &mut AppState| {
            frust::stack()
                .child(tooltip_trigger(
                    &latch,
                    SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                ))
                .child(tooltip(&latch, "Add to library"))
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::ZERO, &mut tcx as &mut dyn Any);
        let mut rec = Recorder::default();
        root.paint(&mut rec, ft_ms(0.0));
        assert!(
            rec.rects.is_empty(),
            "no tip painted for a panel with no extent to hang it off"
        );

        // The same tree at a real viewport still grows one.
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let mut rec = Recorder::default();
        root.paint(&mut rec, ft_ms(16.0));
        assert_eq!(rec.rects.len(), 1, "the tip is back once the panel has one");
    }

    #[test]
    fn the_tip_centres_on_a_panel_shorter_than_the_tip_itself() {
        // Between "no panel at all" and "a panel the tip fits inside" sits a
        // panel too short to hold it: nowhere inside is far enough from both
        // edges, so the tip takes the midpoint rather than a crossed clamp.
        let reach = ARROW_EDGE * std::f64::consts::FRAC_1_SQRT_2;
        // Narrower than the tip's own diagonal, with the trigger below it: the
        // tip goes on the bottom edge and is pinned along `x`, the short axis.
        let panel = Rect::new(100.0, 200.0, 108.0, 228.0);
        let anchor = Rect::new(150.0, 300.0, 230.0, 336.0);
        assert!(panel.width() < 2.0 * reach, "the case this test is about");
        let mut rec = Recorder::default();
        draw_arrow(&mut rec, panel, anchor, Color::BLACK);
        let (center, size, _) = rec.rects[0];
        assert_eq!(
            center,
            Point::new(panel.center().x, panel.y1),
            "centred across the short axis, still on the edge facing the anchor"
        );
        assert_eq!(size.width, 2.0 * reach, "a whole tip, not a shrunken one");
    }

    #[test]
    fn the_tip_is_placed_against_the_trigger_from_a_layer_off_the_window_origin() {
        // The layer's placed rect is lifted into window space for painting, so
        // the anchor it is measured against has to be window space too. Inset
        // the mount and the two spaces stop coinciding.
        const INSET: f64 = 50.0;
        let hover = TooltipHover::new();
        hover.set_open(true);
        let mut root: RenderRoot<AppState, AnyView<AppState>> = RenderRoot::new();
        root.set_theme(Box::new(light()));
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let latch = hover.clone();
        let mut logic = move |_s: &mut AppState| {
            any(frust::Padding(
                frust::EdgeInsets::all(INSET),
                frust::stack()
                    .child(tooltip_trigger(
                        &latch,
                        SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                    ))
                    .child(tooltip(&latch, "Add to library")),
            ))
        };
        // Two frames: the trigger publishes its rect on the first paint, and the
        // layer places against it on the layout that paint asks for.
        for frame in 0..2 {
            root.rebuild(&mut logic, &mut state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft_ms(f64::from(frame) * 16.0));
        }
        let mut rec = Recorder::default();
        root.paint(&mut rec, ft_ms(32.0));

        let trigger = Rect::from_origin_size(Point::new(INSET, INSET), TRIGGER);
        assert_eq!(hover.anchor(), trigger, "the trigger's window-space rect");
        let (center, _, _) = rec.rects[0];
        assert_eq!(
            center.x,
            trigger.center().x,
            "under the trigger, not the layer's own origin away from it"
        );
        // Flush ABOVE the trigger, so the edge facing it is the panel's own
        // `y1` sitting on `trigger.y0`. The panel escapes its padded ancestor
        // onto the window-level overlay, so unlike a mount confined to that
        // ancestor it still has room above the trigger and does not flip below.
        assert_eq!(center.y, trigger.y0, "on the panel edge facing the trigger");
    }

    #[test]
    fn the_panel_is_placed_against_the_captured_trigger_rect() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        assert_eq!(
            h.hover.anchor(),
            Rect::from_origin_size(Point::ORIGIN, TRIGGER),
            "the trigger publishes its own window rect at paint"
        );
    }

    // ---- Typeface: the panel label follows the live theme -----------------

    /// The panel on its own, so the probe needs no hover or anchor to show it.
    #[cfg(feature = "bundled-fonts")]
    fn panel(_: &mut ()) -> TooltipPanel {
        TooltipPanel {
            label: "Add to library".to_string(),
        }
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_panel_label_paints_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a tooltip's panel label",
            panel,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_panel_label_follows_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a tooltip's panel label",
            panel,
        );
    }
}
