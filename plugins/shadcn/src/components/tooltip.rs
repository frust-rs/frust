//! `tooltip`: the hover-opened label that floats beside its trigger — and the
//! **shared hover latch + top layer** the hover card is built on too.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/tooltip.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `Tooltip` portalled to the body at `sideOffset={0}`, whose content is
//! `w-fit rounded-md bg-foreground px-3 py-1.5 text-xs text-background` entering
//! with `animate-in fade-in-0 zoom-in-95`, plus a `size-2.5 rotate-45
//! bg-foreground` arrow tip.
//!
//! # Why this component is a latch and a layer, not an anchored host
//!
//! Every other anchored component in the catalog is opened by a *click*, so the
//! app can own the open flag: a press is an event, an event carries `&mut State`,
//! and the app remounts the overlay on the next frame. A hover-opened overlay
//! cannot work that way, for two reasons:
//!
//! 1. **The delay has no event to fire on.** The open moment is "700ms after the
//!    pointer came to rest", and a resting pointer sends nothing. The only
//!    per-frame pass a widget gets is `paint`, which carries a clock
//!    ([`PaintCtx::frame_time`](frust::authoring::PaintCtx::frame_time)) but no
//!    application state, and the framework exposes no way for a plugin-tier
//!    widget to queue a state-bearing callback onto the next frame. So the open
//!    decision has to live in a widget, not in app state.
//! 2. **A tooltip must not swallow the click it is floating over.**
//!    [`crate::overlay::anchored`] consumes every press outside its content, by
//!    design (that is its light dismiss). Mounted permanently under a hover
//!    trigger it would eat the button press underneath.
//!
//! So the pair here is: [`TooltipHover`], a shared, non-reactive latch holding
//! "is it open" plus the trigger's rect (the same `Rc<Cell<_>>` shape
//! [`OverlayAnchor`] uses, and read the same way — during the layout/paint of the
//! very frame that wrote it); [`tooltip_trigger`], which latches hover, runs the
//! delays off the frame clock, and writes the latch; and [`tooltip`], a full-area
//! **input-transparent** top layer that shows the panel while the latch is set
//! and consumes nothing at all.
//!
//! `on_open_change` is still reported, so an app can mirror the state — but
//! **best-effort and one pass late**: the widget can only call it from an event
//! pass, and the open it is reporting happened during a paint. It is a
//! notification, not the mechanism. Passing [`TooltipTriggerView::open`] a
//! `Some(_)` takes the whole decision over instead (Radix's controlled `open`).
//!
//! # Delays
//!
//! [`TOOLTIP_DELAY_MS`] (700ms) is Radix's own `delayDuration` default;
//! shadcn's `TooltipProvider` overrides it to `0`, which
//! [`TooltipTriggerView::delay`] is one call away from. Closing is immediate for
//! a tooltip and delayed for a hover card ([`HOVER_CARD_CLOSE_DELAY_MS`]).
//! Neither delay is motion: `Theme.motion.reduce_motion` collapses the
//! fade/zoom entrance and leaves both timings alone.
//!
//! Radix's `skipDelayDuration` (a second tooltip inside the same provider opens
//! instantly) has no equivalent here: each trigger owns its own latch, and there
//! is no provider to share a skip window through.
//!
//! # The exit ramp
//!
//! `data-[state=closed]:fade-out-0` is a plain fade with no zoom riding along,
//! and it costs this component nothing to run: the layer is **already mounted
//! permanently**, so the frames the ramp needs are there whether the latch is
//! set or not. A close starts the fade, the layer paints the whole way down, and
//! it stops painting entirely once the presence settles at zero. A hover
//! returning mid-fade picks the entrance up from wherever the presence had got
//! to, and `reduce_motion` collapses both directions to a jump.
//!
//! A closing panel consumes nothing, exactly as an open one does not — the layer
//! is input-transparent by contract — and its `over_panel` grip is dropped with
//! the latch, so a fading panel cannot hold its own card open.
//!
//! # Touch
//!
//! Touch has no hover, so a tooltip never opens on a touch device.
//! Long-press-opens-a-tooltip is not modelled.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, SemanticsCtx, Size, ThemeTextColor, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{AnimationController, Curve, FrameTime, Theme, text};

use crate::hit::inside;
use crate::overlay::{OverlayAlign, OverlayPlacement, OverlaySide, finite_or_zero, place};
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
/// `duration-200`, shared with every other overlay's entrance.
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
/// asked for the next frame, and the layer reads it during the layout and paint
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
        // space, which is what the layer's placement needs.
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

/// The chrome a [`TooltipLayerView`] paints around its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TooltipArrow {
    /// The tooltip's `size-2.5 rotate-45 bg-foreground` tip.
    Tip,
    /// No tip (the hover card has none).
    None,
}

/// Build a tooltip layer showing `label` while `hover` is open.
///
/// Mount it as the top child of a full-area [`frust::Stack`], **permanently** —
/// it paints nothing while closed and consumes no input ever, so it costs a
/// layout of its content and nothing else. That stack must itself be bounded:
/// inside a scroll view the layer has no vertical area to place a panel in
/// (`crate::overlay`'s `finite_or_zero`, the scroll-view trap).
pub fn tooltip<State: 'static>(
    hover: &TooltipHover,
    label: impl Into<String>,
) -> TooltipLayerView<State> {
    TooltipLayerView {
        content: any(TooltipPanel {
            label: label.into(),
        }),
        hover: hover.clone(),
        // `sideOffset={0}` — the tooltip sits flush against its trigger, with the
        // arrow tip bridging the gap.
        placement: OverlayPlacement::on(OverlaySide::Top).offset(0.0),
        arrow: TooltipArrow::Tip,
    }
}

/// Build a layer showing `content` while `hover` is open — the hover card's half
/// of the shared machinery. See [`tooltip`].
pub(crate) fn tooltip_layer<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    content: V,
) -> TooltipLayerView<State> {
    TooltipLayerView {
        content: any(content),
        hover: hover.clone(),
        placement: OverlayPlacement::default(),
        arrow: TooltipArrow::None,
    }
}

/// A declarative hover-overlay layer. See [`tooltip`].
pub struct TooltipLayerView<State: 'static> {
    content: AnyView<State>,
    hover: TooltipHover,
    placement: OverlayPlacement,
    arrow: TooltipArrow,
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

/// The retained widget for a [`TooltipLayerView`].
pub struct TooltipLayerWidget {
    content: ChildPod,
    hover: TooltipHover,
    placement: OverlayPlacement,
    arrow: TooltipArrow,
    /// The placed panel rect, in this layer's own space.
    rect: Rect,
    /// The anchor the last layout placed against, and this layer's own absolute
    /// origin — the same self-correcting pair
    /// [`crate::overlay::anchored`](crate::overlay::AnchoredOverlayWidget) keeps.
    anchor_used: Rect,
    host_origin: Point,
    /// Whether the panel was showing on the last paint (an edge starts a ramp,
    /// in whichever direction the edge went).
    was_open: bool,
    anim: AnimationController,
    /// How present the panel is: `0.0` gone, `1.0` settled.
    presence: f64,
}

impl TooltipLayerWidget {
    /// The placed panel rect, in the layer's own coordinate space.
    pub fn panel_rect(&self) -> Rect {
        self.rect
    }

    /// How present the panel is — `0.0` gone, `1.0` settled.
    pub fn progress(&self) -> f64 {
        self.presence
    }

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
            // close delays above are timing, not motion, and are untouched.
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

impl<State: 'static> View<State> for TooltipLayerView<State> {
    type Element = TooltipLayerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipLayerWidget {
        TooltipLayerWidget {
            content: build_child(&self.content, ctx),
            hover: self.hover.clone(),
            placement: self.placement,
            arrow: self.arrow,
            rect: Rect::ZERO,
            anchor_used: Rect::ZERO,
            host_origin: Point::ORIGIN,
            was_open: false,
            anim: AnimationController::new(Duration::from_millis(ENTRANCE_MS))
                .with_curve(Curve::EaseOut),
            presence: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipLayerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.placement != self.placement {
            element.placement = self.placement;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.hover = self.hover.clone();
        element.arrow = self.arrow;
        flags
    }

    fn teardown(&self, element: &mut TooltipLayerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for TooltipLayerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        let content = self.content.layout_child(ctx, &BoxConstraints::loose(area));
        self.anchor_used = self.hover.anchor();
        // Window space → this layer's own space (the identity in the supported
        // mount, the top child of a full-area `Stack`).
        let anchor_local = self.anchor_used - self.host_origin.to_vec2();
        self.rect = place(
            anchor_local,
            content,
            Rect::from_origin_size(Point::ORIGIN, area),
            self.placement,
        );
        self.content.set_origin(self.rect.origin());
        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The same self-correction the anchored host runs: this is the only pass
        // that knows the layer's absolute origin, and the latch can move under a
        // layout that already ran.
        if ctx.origin() != self.host_origin || self.hover.anchor() != self.anchor_used {
            self.host_origin = ctx.origin();
            ctx.request_layout();
        }
        let open = self.hover.is_open();
        // Settled closed: paint nothing. The layer stays mounted, costing a
        // layout of its content and this check. `was_open` is part of the test
        // because a latch that closes on the frame after it opened leaves the
        // presence at zero with the edge still owed — skipping the ramp there
        // would strand `was_open` set and the next open would never start.
        if !open && !self.was_open && self.presence <= 0.0 {
            return;
        }
        let (reduce_motion, fill) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                crate::overlay::foreground(theme),
            )
        };
        let presence = self.ramp(ctx, open, reduce_motion);
        if !open && presence <= 0.0 {
            // The fade-out has just settled.
            return;
        }

        let origin = ctx.origin();
        let panel = self.rect + origin.to_vec2();
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
            draw_arrow(scene, panel, self.anchor_used - origin.to_vec2(), fill);
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
        // Input-transparent by contract: the layer observes moves to keep the
        // latch's "pointer is over the panel" flag current and **consumes
        // nothing**, so the trigger underneath keeps seeing its own hover and a
        // press anywhere reaches whatever is below.
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Move
        {
            let over = self.hover.is_open() && self.rect.contains(p.position);
            if over != self.hover.over_panel() {
                self.hover.set_over_panel(over);
                ctx.request_redraw();
            }
            if over {
                // Claimed so nothing under the panel lights up while the pointer
                // is over it; the trigger stays "active" through the latch's own
                // `over_panel` flag rather than through the hover link.
                ctx.claim_hover();
            }
        }
        // The content is never routed to: a hover overlay has no interactive
        // parts in v1, and routing would make it possible for the panel to
        // swallow a press meant for the page.
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.hover.is_open() {
            self.content.semantics_child(ctx);
        }
    }

    visit_children!(content);
}

/// Paint the tooltip's `size-2.5 rotate-45` tip on the panel edge facing the
/// anchor.
fn draw_arrow(scene: &mut dyn PaintScene, panel: Rect, anchor: Rect, color: Color) {
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
        Point::new(center.x, center.y.clamp(panel.y0 + reach, panel.y1 - reach))
    } else {
        Point::new(center.x.clamp(panel.x0 + reach, panel.x1 - reach), center.y)
    };
    let mut path = BezPath::new();
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
    /// The label child.
    fn label_view<State: 'static>(&self) -> AnyView<State> {
        any(text(self.label.clone())
            .size(style::TEXT_XS as f32)
            .family(crate::tokens::sans_family())
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
    }

    const TRIGGER: Size = Size::new(80.0, 36.0);

    /// The mount an app uses: the trigger in the page, the layer as the top child
    /// of a full-area [`frust::Stack`], permanently.
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
                frust::Stack(vec![
                    any(tooltip_trigger(
                        &hover,
                        SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                    )
                    .open(controlled)
                    .on_open_change(|s: &mut AppState, open| s.opens.push(open))),
                    any(tooltip(&hover, "Add to library")),
                ])
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
    fn the_layer_paints_nothing_while_closed_and_a_panel_with_a_tip_while_open() {
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
    fn the_layer_consumes_nothing_so_the_page_under_it_keeps_its_input() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());
        let panel = {
            let rect = place(
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

    #[test]
    fn an_unbounded_height_collapses_the_layer_and_clamps_the_panel_to_its_top() {
        // The documented mount is a full-area `Stack`, which is bounded; a
        // layer put inside a scroll view is not, and this is what that costs —
        // pinned, not fixed here: the coercion cannot invent an extent nobody
        // offered, so the fix belongs at the mount site.
        let hover = TooltipHover::new();
        let trigger = Rect::from_origin_size(Point::new(40.0, 300.0), TRIGGER);
        hover.set_anchor(trigger);
        let view: TooltipLayerView<AppState> = tooltip(&hover, "Add to library");
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        // The constraints `frust::scroll_view` hands its child: the viewport
        // width, an infinite max height.
        let scrolled = BoxConstraints::new(
            Size::new(WINDOW.width, 0.0),
            Size::new(WINDOW.width, f64::INFINITY),
        );
        let size = w.layout(&mut lctx, &scrolled);
        assert_eq!(size.height, 0.0, "no vertical area to fill");
        assert_eq!(
            w.panel_rect().y0,
            0.0,
            "clamped to the top of a zero-height area, nowhere near its trigger"
        );

        // The same layer under the contract's own constraints places normally.
        let bounded = w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        assert_eq!(bounded, WINDOW);
        assert_eq!(w.panel_rect().y1, trigger.y0, "flush above its trigger");
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
}
