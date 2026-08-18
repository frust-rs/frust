//! `popover`: a trigger-anchored panel opened by a click, plus the **shared
//! anchored-panel chrome** every other anchored component in the catalog paints
//! through.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/popover.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `Popover` portalled to the body and positioned against its trigger, whose
//! content is `z-50 w-72 rounded-md border bg-popover p-4 text-popover-foreground
//! shadow-md`, entering with `fade-in-0 zoom-in-95` and leaving with
//! `fade-out-0 zoom-out-95`.
//!
//! # Shape of the port
//!
//! frust has no portal, so the panel mounts through [`crate::overlay::anchored`]
//! — a full-area top-layer host that places the panel against the rect an
//! [`OverlayAnchor`] captured from the trigger (see that module for the two
//! supported mounts and the light-dismiss contract). [`popover`] is a thin
//! wrapper over that host: it builds the panel, hands the host the placement, and
//! maps the host's dismiss onto `on_open_change(false)`.
//!
//! Open/close is **controlled**: the app owns the flag and gets every close
//! request (a press outside, Escape) through `on_open_change`. There is no
//! uncontrolled mode — a widget cannot open itself.
//!
//! Two mounts are supported, and [`crate::overlay::anchored`] documents both.
//! Mounting the popover only while the flag is set is the simple one; mounting
//! it always and handing the flag down through [`PopoverView::open`] is the
//! **kept-mounted** pattern, and the only one that can play an exit ramp (see
//! the panel section below).
//!
//! # The shared panel
//!
//! [`PanelStyle`]/[`panel`] (crate-internal) are the chrome the whole anchored
//! family shares: the popover, the hover card, the dropdown/context menus, the
//! select list and the combobox all paint the same `rounded-md border bg-popover
//! shadow-md` box with the same `fade-in-0 zoom-in-95` entrance, and differ only
//! in padding and width. They live here because the popover is the family's
//! archetype, exactly as `native_select` owns the shared chevron.
//!
//! `data-[side=*]:slide-in-from-*` — the 8px directional slide that rides along
//! with the entrance — is deliberately not modelled: the host resolves the side
//! (it may flip), and the panel is not told which side it landed on, so the
//! slide has no direction to take. The fade and the zoom carry the entrance on
//! their own.
//!
//! # The exit ramp
//!
//! `data-[state=closed]:fade-out-0 zoom-out-95` is the entrance played
//! backwards, and the panel runs it off the same frame clock. A panel handed
//! `open == false` ramps its presence down to zero over the same 200ms, goes on
//! painting the whole way, and paints nothing at all once it settles. Reopening
//! mid-ramp reverses from wherever the presence had got to rather than snapping
//! back to transparent, and `reduce_motion` collapses both directions to a jump.
//!
//! **A closing panel is inert.** It claims no hover, takes no key and consumes no
//! press, so a click during the ramp lands on whatever is under it — a closing
//! overlay must never eat the interaction that follows the close.
//!
//! This needs the kept-mounted pattern: a panel the app unmounts on close has no
//! frames left to ramp in, and the exit is simply truncated (see
//! [`crate::overlay::anchored`]'s mounting docs). Either way the close is
//! reported to the app **immediately** — `on_open_change(false)` fires from the
//! dismiss event itself, so app state is never lagging behind the animation; it
//! is only the pixels that linger.

use std::cell::RefCell;
use std::rc::Rc;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{AnimationController, Curve, Theme};
use std::time::Duration;

use crate::hit::inside;
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchor as capture_anchor, anchored,
};
use crate::style::{self, PATH_TOLERANCE, ShadcnShadow};
use crate::tokens::ShadcnTokens;

/// `w-72` — the popover panel's width, in logical px.
pub const POPOVER_WIDTH: f64 = 288.0;
/// `p-4` — the popover/hover-card panel's padding, in logical px.
pub const POPOVER_PADDING: f64 = 16.0;
/// `p-1` — the padding of a panel whose own rows carry the inset (the menus, the
/// select list).
pub(crate) const MENU_PADDING: f64 = 4.0;
/// `min-w-[8rem]` — the menu/select panel's minimum width, in logical px.
pub(crate) const MIN_MENU_WIDTH: f64 = 128.0;

/// `duration-200` — the entrance ramp, shared with the modal family. The exit
/// (`data-[state=closed]:*`) is the same ramp run backwards.
const ENTRANCE_MS: u64 = 200;
/// `zoom-in-95`/`zoom-out-95` — the scale the ramp starts from and returns to.
const ZOOM_FROM: f64 = 0.95;
/// Below this the progress is treated as unchanged (no frame is asked for).
const PROGRESS_EPSILON: f64 = 1e-4;
/// How far outside the panel the composited entrance layer reaches, so the
/// `shadow-md` under it is not clipped out of the fade.
const SHADOW_SPILL: f64 = 32.0;
/// Panel width used when the incoming constraints are horizontally unbounded —
/// a host always gives bounded ones, so this only guards the degenerate case.
const UNBOUNDED_WIDTH: f64 = 320.0;

/// The chrome of one anchored panel: padding, how wide it gets, and which shadow
/// rung it sits on.
///
/// Shared by every anchored component (see the [module docs](self)); a component
/// starts from one of the constructors below and adjusts.
#[derive(Clone, Debug)]
pub(crate) struct PanelStyle {
    /// Horizontal padding, in logical px.
    pub pad_x: f64,
    /// Vertical padding, in logical px.
    pub pad_y: f64,
    /// A fixed width (`w-72`, `w-64`), or `None` to size to the content.
    pub width: Option<f64>,
    /// The floor a content-sized panel keeps (`min-w-[8rem]`).
    pub min_width: f64,
    /// The trigger whose width the panel matches as a second floor — the select
    /// list's `min-w-[var(--radix-select-trigger-width)]`.
    pub anchor: Option<OverlayAnchor>,
    /// The panel's shadow rung.
    pub shadow: ShadcnShadow,
    /// Whether the panel runs the `fade-in-0 zoom-in-95` ramp at all. A panel
    /// whose parent composites it (the hover card, inside the tooltip layer)
    /// sets this `false` and is always fully present.
    pub entrance: bool,
    /// Whether the panel is open. Flipping it to `false` on a **mounted** panel
    /// starts the exit ramp (see the [module docs](self)); the default `true` is
    /// the mount-on-open contract.
    pub open: bool,
}

impl PanelStyle {
    /// `w-72 p-4 shadow-md` — the popover's own chrome.
    pub fn popover() -> Self {
        PanelStyle {
            pad_x: POPOVER_PADDING,
            pad_y: POPOVER_PADDING,
            width: Some(POPOVER_WIDTH),
            min_width: 0.0,
            anchor: None,
            shadow: style::SHADOW_MD,
            entrance: true,
            open: true,
        }
    }

    /// `min-w-32 p-1 shadow-md` — the menu/select panel's chrome.
    pub fn menu() -> Self {
        PanelStyle {
            pad_x: MENU_PADDING,
            pad_y: MENU_PADDING,
            width: None,
            min_width: MIN_MENU_WIDTH,
            anchor: None,
            shadow: style::SHADOW_MD,
            entrance: true,
            open: true,
        }
    }

    /// The width floor this style resolves for the current trigger rect.
    fn floor(&self) -> f64 {
        let anchor = self.anchor.as_ref().map_or(0.0, |a| a.rect().width());
        self.min_width.max(anchor)
    }
}

/// A live panel style, shared between a component's builder and the panel view it
/// already wrapped its content in.
///
/// The content is erased into an [`AnyView`] at construction and cannot be
/// rebuilt from a `&self`, so a builder call landing *after* that (`.width(..)`,
/// `.anchor(..)`) reaches the panel through this handle rather than by
/// re-wrapping.
pub(crate) type PanelHandle = Rc<RefCell<PanelStyle>>;

/// Wrap `content` in the shared anchored-panel chrome carried by `style`.
pub(crate) fn panel<State: 'static, V: View<State>>(
    content: V,
    style: PanelHandle,
) -> PanelView<State> {
    PanelView {
        content: any(content),
        style,
    }
}

/// A declarative anchored panel. See [`panel`].
pub(crate) struct PanelView<State: 'static> {
    content: AnyView<State>,
    style: PanelHandle,
}

/// The retained widget for a [`PanelView`].
pub(crate) struct PanelWidget {
    content: ChildPod,
    style: PanelStyle,
    anim: AnimationController,
    /// Whether the mount pass has taken its first ramp decision.
    started: bool,
    /// The open flag the last paint ramped toward — a change is what starts the
    /// entrance or the exit.
    was_open: bool,
    /// How present the panel is: `0.0` gone, `1.0` settled. One value for both
    /// directions, since the exit is the entrance backwards.
    presence: f64,
}

impl PanelWidget {
    /// How present the panel is — `0.0` gone, `1.0` settled. Introspection for
    /// this crate's own tests.
    #[cfg(test)]
    pub(crate) fn progress(&self) -> f64 {
        self.presence
    }

    /// Take this paint's ramp decision, returning the presence to paint at.
    ///
    /// Split out of [`Widget::paint`] so the paint arm reads as chrome: this is
    /// the whole open/close state machine, and it is the only thing that touches
    /// the controller.
    fn ramp(&mut self, ctx: &mut PaintCtx, reduce_motion: bool) -> f64 {
        let open = self.style.open;
        // A panel that mounts already closed has nothing to ramp out of, so it
        // joins the two standing instant cases rather than running a no-op
        // reverse that asks for 200ms of frames.
        let instant = reduce_motion || !self.style.entrance || (!self.started && !open);
        if !self.started || open != self.was_open {
            self.started = true;
            self.was_open = open;
            if instant {
                self.anim.stop();
                self.presence = if open { 1.0 } else { 0.0 };
            } else if open {
                // Both start from the controller's current value, so reopening
                // mid-exit reverses from where the ramp had got to.
                self.anim.forward();
            } else {
                self.anim.reverse();
            }
        }
        if instant {
            // `reduce_motion` collapses either direction to a jump and stops
            // asking for frames.
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

impl<State: 'static> View<State> for PanelView<State> {
    type Element = PanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PanelWidget {
        let style = self.style.borrow().clone();
        PanelWidget {
            content: build_child(&self.content, ctx),
            anim: AnimationController::new(Duration::from_millis(ENTRANCE_MS))
                .with_curve(Curve::EaseOut),
            started: false,
            // Seeded closed so the first paint's ramp decision reads as an
            // open edge and runs the entrance.
            was_open: false,
            presence: 0.0,
            style,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        let style = self.style.borrow().clone();
        if style.pad_x != element.style.pad_x
            || style.pad_y != element.style.pad_y
            || style.width != element.style.width
            || style.min_width != element.style.min_width
        {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if style.open != element.style.open {
            // The paint is what starts the ramp, in either direction.
            flags |= ChangeFlags::PAINT;
        }
        element.style = style;
        flags
    }

    fn teardown(&self, element: &mut PanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for PanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };
        let outer = self.style.width.unwrap_or(available).min(available);
        let inner_max = (outer - 2.0 * self.style.pad_x).max(0.0);
        let content = self.content.layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(inner_max, f64::INFINITY)),
        );
        let width = match self.style.width {
            Some(fixed) => fixed.min(available),
            None => (content.width + 2.0 * self.style.pad_x)
                .max(self.style.floor())
                .min(available),
        };
        self.content
            .set_origin(Point::new(self.style.pad_x, self.style.pad_y));
        bc.constrain(Size::new(width, content.height + 2.0 * self.style.pad_y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        // One scope for every theme read: a live `&Theme` borrows the context,
        // and the frame requests plus the child paint below need it mutably.
        let (reduce_motion, radius, fill, border, shadow) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ShadcnTokens::resolve_radius(None, theme).md,
                crate::overlay::popover(theme),
                crate::overlay::border(theme),
                self.style.shadow.color(theme),
            )
        };

        let presence = self.ramp(ctx, reduce_motion);
        if !self.style.open && presence <= 0.0 {
            // Settled closed: the exit ramp is over and there is nothing left to
            // draw. The widget stays mounted, costing a layout and this check.
            return;
        }

        let ramping = presence < 1.0;
        if ramping {
            // `fade-in-0`/`fade-out-0`: composite the panel at the ramp's alpha,
            // over a layer inflated so the shadow that reaches outside it is not
            // clipped away.
            let layer = Rect::from_origin_size(origin, size).inflate(SHADOW_SPILL, SHADOW_SPILL);
            scene.push_layer(layer.origin(), layer.size(), presence as f32);
            // …and `zoom-in-95`/`zoom-out-95`: scale about the panel's own centre.
            let c = Rect::from_origin_size(origin, size).center();
            let scale = ZOOM_FROM + (1.0 - ZOOM_FROM) * presence;
            scene.push_transform(
                Affine::translate(c.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-c.to_vec2()),
            );
        }

        scene.draw_shadow(
            Point::new(origin.x, origin.y + self.style.shadow.y_offset),
            size,
            radius,
            self.style.shadow.std_dev,
            shadow,
        );
        scene.fill_rounded_rect(origin, size, radius, fill);
        // `overflow-hidden`: rows are clipped to the panel's rounded box.
        scene.push_clip_rounded(origin, size, radius);
        self.content.paint_child(ctx, scene);
        scene.pop_clip();
        stroke_panel_border(scene, origin, size, radius, border);

        if ramping {
            scene.pop_transform();
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A broadcast reaches the content whatever the panel's state — it is
        // never consumed, and it is what keeps the pods live.
        if event.is_broadcast() {
            return route_event_single(&mut self.content, ctx, event);
        }
        // A closed or closing panel is inert (the module docs' exit ramp): it
        // goes on painting through the ramp but claims no hover and consumes no
        // press, so a click during the exit lands on what is under it.
        if !self.style.open {
            return EventResult::Ignored;
        }
        // A focus-routed event reaches the content unconditionally. The panel is
        // pure chrome around exactly one child and never focuses itself, so the
        // focus-path gate the shared helper applies would strand a key in a panel
        // nothing has pressed inside — which is every chained submenu, opened by
        // hover or by `ArrowRight` and never pressed at all. Whether the panel
        // hears the key in the first place is still its own parent's decision.
        if event.is_focus_routed() {
            return self.content.event_child(ctx, event);
        }
        // Everything else: the content sees the pass first, and the panel claims
        // nothing of its own, so the claim-ordering rule holds by construction.
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A panel on its way out is not there to be read.
        if self.style.open {
            self.content.semantics_child(ctx);
        }
    }

    visit_children!(content);
}

/// Stroke a panel's 1px `border` around `origin`/`size`.
pub(crate) fn stroke_panel_border(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    color: Color,
) {
    let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
    scene.stroke_path(
        origin,
        &rr.to_path(PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

/// A view-held, typed open-change callback (erased on build).
pub(crate) type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative shadcn popover. See [`popover`].
pub struct PopoverView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    style: PanelHandle,
    placement: OverlayPlacement,
}

/// Build a popover panel showing `content`, to be mounted while the app's own
/// open flag is set (see the [module docs](self)).
///
/// Chain [`PopoverView::anchor`] to point it at the trigger's captured rect and
/// [`PopoverView::on_open_change`] to hear about a close request.
pub fn popover<State: 'static, V: View<State>>(content: V) -> PopoverView<State> {
    let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::popover()));
    PopoverView {
        inner: anchored(panel(content, style.clone())),
        style,
        placement: OverlayPlacement::default(),
    }
}

impl<State: 'static> PopoverView<State> {
    /// Anchor the panel to the rect `anchor` carries.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the panel opens on (`side`, default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.apply_placement()
    }

    /// Set the cross-axis alignment (`align`, default `center`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.apply_placement()
    }

    /// Set the gap between trigger and panel (`sideOffset`, default `4`).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self.apply_placement()
    }

    /// Override the panel's `w-72` width.
    pub fn width(self, width: f64) -> Self {
        self.style.borrow_mut().width = Some(width);
        self
    }

    /// Hand a **kept-mounted** popover the app's open flag, so closing it plays
    /// the exit ramp instead of vanishing (see the [module docs](self)).
    ///
    /// The default is `true`: a mounted popover is an open one, which is what a
    /// mount-on-open app wants.
    pub fn open(mut self, open: bool) -> Self {
        self.style.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback. The host reports a press outside the panel
    /// and a focus-routed Escape as `on_open_change(state, false)`; nothing here
    /// ever reports `true` (a mounted panel is already open).
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }

    /// Re-hand the current placement to the host.
    fn apply_placement(mut self) -> Self {
        self.inner = self.inner.placement(self.placement);
        self
    }
}

impl<State: 'static> View<State> for PopoverView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

/// Wrap `child` as an overlay trigger: it captures its own rect into `anchor` and
/// reports a click as an open toggle.
///
/// The wrapper is transparent — it lays out, paints and publishes the semantics
/// of `child` unchanged — and defers to it: a child that consumes the press (a
/// [`crate::button`], which has an `on_click` of its own) keeps it, and the
/// wrapper fires nothing. That is what lets *any* view be a trigger while a
/// control that already knows how to be pressed keeps its own behavior.
///
/// Shared by the popover, both menus and the combobox; the select ships its own
/// trigger because its chrome is part of the component.
pub fn popover_trigger<State: 'static, V: View<State>>(
    anchor: &OverlayAnchor,
    child: V,
) -> PopoverTriggerView<State> {
    PopoverTriggerView {
        child: any(capture_anchor(anchor, child)),
        open: false,
        on_open_change: Rc::new(|_, _| {}),
    }
}

/// A declarative overlay trigger. See [`popover_trigger`].
pub struct PopoverTriggerView<State: 'static> {
    child: AnyView<State>,
    open: bool,
    on_open_change: OnOpenChange<State>,
}

impl<State: 'static> PopoverTriggerView<State> {
    /// Tell the trigger whether its overlay is currently open, so a click can
    /// report the *other* state (Radix's toggle-on-trigger behavior).
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the open-change callback: `!open` on a release inside the trigger.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// The retained widget for a [`PopoverTriggerView`].
pub struct PopoverTriggerWidget {
    child: ChildPod,
    open: bool,
    captured: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for PopoverTriggerView<State> {
    type Element = PopoverTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PopoverTriggerWidget {
        PopoverTriggerWidget {
            child: build_child(&self.child, ctx),
            open: self.open,
            captured: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PopoverTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        element.open = self.open;
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut PopoverTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for PopoverTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The child owns the pass: a broadcast must reach it, and a pressable
        // child keeps its own press.
        let routed = route_event_single(&mut self.child, ctx, event);
        if event.is_broadcast() || routed == EventResult::Handled {
            return routed;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let inside = inside(p.position, ctx.size());
        match p.phase {
            PointerPhase::Down if inside => {
                self.captured = true;
                ctx.capture_pointer();
                // Focus is what routes Escape to the overlay's host afterwards.
                ctx.request_focus();
                EventResult::Handled
            }
            PointerPhase::Move if !self.captured && inside => {
                // Claimed after the routing above (the claim-ordering rule).
                ctx.claim_hover();
                ctx.set_cursor(style::ACTIVE_CURSOR);
                EventResult::Ignored
            }
            PointerPhase::Up if self.captured => {
                self.captured = false;
                if inside {
                    let next = !self.open;
                    (self.on_open_change)(ctx, next);
                }
                EventResult::Handled
            }
            // A `Cancel` arm clears its own flag and touches no app state.
            PointerPhase::Cancel if self.captured => {
                self.captured = false;
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
    };
    use frust::{Brightness, FrameTime, SizedBox};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A recording [`PaintScene`] shared by every anchored component's tests.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub rects: Vec<(Point, Size, Color)>,
        pub rrects: Vec<(Point, Size, f64, Color)>,
        pub strokes: Vec<(Rect, f64, Color)>,
        pub shadows: Vec<(Point, Size, f64, f64, Color)>,
        pub layers: Vec<f32>,
        pub transforms: Vec<Affine>,
        pub texts: Vec<String>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, text: &str) {
            self.texts.push(text.to_string());
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.rects.push((origin, path.bounding_box().size(), color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn draw_shadow(
            &mut self,
            origin: Point,
            size: Size,
            radius: f64,
            std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, size, radius, std_dev, color));
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {}
    }

    /// The window every anchored component's tests lay out in.
    pub(crate) const WINDOW: Size = Size::new(400.0, 600.0);

    /// A `FrameTime` `ms` milliseconds after zero.
    pub(crate) fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A primary-button pointer event at `(x, y)`.
    pub(crate) fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// A key event carrying `key`.
    pub(crate) fn key_event(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// The Escape key event.
    pub(crate) fn escape() -> InputEvent {
        key_event(Key::Named(NamedKey::Escape))
    }

    /// The shadcn theme in light mode — what every chrome assertion resolves
    /// against.
    pub(crate) fn light() -> Theme {
        crate::theme().with_brightness(Brightness::Light)
    }

    #[derive(Default)]
    struct AppState {
        open: bool,
        opens: Vec<bool>,
    }

    const TRIGGER: Size = Size::new(80.0, 36.0);
    const CONTENT: Size = Size::new(120.0, 40.0);

    /// The mount an app uses: the trigger in the page, the panel as the top
    /// child of a full-area [`frust::Stack`].
    ///
    /// `kept` picks between the two mount contracts — mount-on-open (the panel
    /// is in the tree only while the flag is set) and kept-mounted (it is always
    /// in the tree, handed the flag through `open`), which is the one an exit
    /// ramp needs.
    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
        kept: bool,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_mount(false)
        }

        /// A kept-mounted harness — the pattern an exit ramp requires.
        fn kept_mounted() -> Self {
            Self::with_mount(true)
        }

        fn with_mount(kept: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor: OverlayAnchor::new(),
                kept,
                clock: 0.0,
            };
            h.root.set_theme(Box::new(light()));
            h.pass();
            h
        }

        fn pass(&mut self) {
            let anchor = self.anchor.clone();
            let kept = self.kept;
            let mut logic = move |state: &mut AppState| {
                let trigger =
                    popover_trigger(&anchor, SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)))
                        .open(state.open)
                        .on_open_change(|s: &mut AppState, open| {
                            s.opens.push(open);
                            s.open = open;
                        });
                let mut children = vec![any(trigger)];
                if kept || state.open {
                    children.push(any(popover(SizedBox(
                        Some(CONTENT.width),
                        Some(CONTENT.height),
                    ))
                    .anchor(&anchor)
                    .width(CONTENT.width + 2.0 * POPOVER_PADDING)
                    .open(state.open)
                    .on_open_change(|s: &mut AppState, open| {
                        s.opens.push(open);
                        s.open = open;
                    })));
                }
                frust::Stack(children)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        /// Paint at `ms` without rebuilding — the ramp's own frames.
        fn paint_at(&mut self, ms: f64) -> Recorder {
            self.clock = ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        fn click(&mut self, x: f64, y: f64) {
            self.event(pointer(PointerPhase::Down, x, y));
            self.event(pointer(PointerPhase::Up, x, y));
            self.pass();
        }
    }

    #[test]
    fn a_click_on_the_trigger_opens_and_a_second_one_closes() {
        let mut h = Harness::new();
        assert!(!h.state.open);
        h.click(10.0, 10.0);
        assert_eq!(h.state.opens, vec![true], "up-inside reports the toggle");
        assert!(h.state.open);

        // The trigger sits under the host now; a click on it still toggles,
        // because the host consumes only presses outside its own content and the
        // trigger is outside it — so this arrives as a dismiss.
        h.click(10.0, 10.0);
        assert_eq!(h.state.opens, vec![true, false]);
        assert!(!h.state.open);
    }

    #[test]
    fn a_press_outside_the_panel_closes_it_and_never_reaches_the_page() {
        let mut h = Harness::new();
        h.click(10.0, 10.0);
        assert!(h.state.open);
        h.event(pointer(PointerPhase::Down, 380.0, 580.0));
        assert_eq!(h.state.opens, vec![true, false]);
    }

    #[test]
    fn escape_closes_the_panel_once_a_press_has_focused_the_host() {
        let mut h = Harness::new();
        h.click(10.0, 10.0);
        let panel = h.anchor.rect();
        let inside = Point::new(panel.center().x, panel.y1 + 20.0);
        h.event(pointer(PointerPhase::Down, inside.x, inside.y));
        h.event(pointer(PointerPhase::Up, inside.x, inside.y));
        h.event(escape());
        assert!(!h.state.opens.last().unwrap(), "the last report is a close");
    }

    #[test]
    fn the_panel_is_placed_below_the_trigger_and_flips_near_the_bottom_edge() {
        let mut h = Harness::new();
        h.click(10.0, 10.0);
        let placed = crate::overlay::place(
            h.anchor.rect(),
            Size::new(
                CONTENT.width + 2.0 * POPOVER_PADDING,
                CONTENT.height + 2.0 * POPOVER_PADDING,
            ),
            Rect::from_origin_size(Point::ORIGIN, WINDOW),
            OverlayPlacement::default(),
        );
        assert_eq!(placed.y0, h.anchor.rect().y1 + crate::overlay::SIDE_OFFSET);

        // A trigger near the bottom edge flips the panel above itself.
        let low = Rect::from_origin_size(Point::new(10.0, 580.0), TRIGGER);
        let flipped = crate::overlay::place(
            low,
            Size::new(CONTENT.width, CONTENT.height),
            Rect::from_origin_size(Point::ORIGIN, WINDOW),
            OverlayPlacement::default(),
        );
        assert_eq!(flipped.y1, low.y0 - crate::overlay::SIDE_OFFSET);
    }

    #[test]
    fn the_panel_paints_popover_chrome_at_the_shared_radius() {
        let theme = light();
        let mut view: PopoverView<AppState> = popover(SizedBox(Some(120.0), Some(40.0)));
        view = view.width(POPOVER_WIDTH);
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        let mut rec = Recorder::default();
        // A settled pass: the second paint is past the ramp's end.
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(0.0)).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ENTRANCE_MS as f64 * 2.0))
            .with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);

        let (origin, size, radius, color) = rec.rrects[0];
        assert_eq!(color, theme.scheme().surface_container_high, "bg-popover");
        assert_eq!(size.width, POPOVER_WIDTH, "w-72");
        assert_eq!(radius, ShadcnTokens::shadcn().radius.md);
        assert_eq!(
            rec.shadows[0].4.components[3],
            style::SHADOW_MD.alpha,
            "shadow-md"
        );
        assert!(
            rec.strokes
                .iter()
                .any(|(_, w, c)| *w == style::BORDER_WIDTH && *c == theme.scheme().outline),
            "the 1px border"
        );
        assert!(size.height >= 40.0 + 2.0 * POPOVER_PADDING, "p-4");
        assert!(origin.x >= 0.0);
    }

    #[test]
    fn the_entrance_fades_and_zooms_then_settles_and_reduce_motion_jumps() {
        let build_panel = || {
            let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::popover()));
            let view: PanelView<AppState> = panel(SizedBox(Some(120.0), Some(40.0)), style);
            let mut counter = 0u64;
            let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size = w.layout(&mut lctx, &BoxConstraints::loose(WINDOW));
            (w, size)
        };

        let (mut w, size) = build_panel();
        let frame = |w: &mut PanelWidget, ms: f64, theme: &Theme| {
            let mut rec = Recorder::default();
            let mut ctx =
                PaintCtx::for_test(Point::ORIGIN, size, ft_ms(ms)).with_theme(theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
            rec
        };
        let theme = light();
        let first = frame(&mut w, 0.0, &theme);
        assert_eq!(first.layers, vec![0.0], "starts transparent");
        let mid = frame(&mut w, ENTRANCE_MS as f64 / 2.0, &theme);
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);
        assert_eq!(mid.transforms.len(), 1, "and a zoom");
        let done = frame(&mut w, ENTRANCE_MS as f64 * 2.0, &theme);
        assert!(done.layers.is_empty() && done.transforms.is_empty());
        assert!((w.progress() - 1.0).abs() < 1e-9);

        let mut reduced = light();
        reduced.motion.reduce_motion = true;
        let (mut w, _) = build_panel();
        let jumped = frame(&mut w, 0.0, &reduced);
        assert!(jumped.layers.is_empty(), "no fade at all");
        assert!((w.progress() - 1.0).abs() < 1e-9);
    }

    /// A panel over a live style handle, plus the rebuild that re-reads it —
    /// the seam a component's `open` flag reaches a mounted panel through.
    struct PanelHarness {
        view: PanelView<AppState>,
        widget: PanelWidget,
        size: Size,
        counter: u64,
    }

    impl PanelHarness {
        fn new(style: PanelHandle) -> Self {
            let view: PanelView<AppState> = panel(SizedBox(Some(120.0), Some(40.0)), style);
            let mut counter = 0u64;
            let mut widget = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size = widget.layout(&mut lctx, &BoxConstraints::loose(WINDOW));
            PanelHarness {
                view,
                widget,
                size,
                counter,
            }
        }

        /// Re-read the style handle, the way an app's rebuild does. The view is
        /// its own previous self: only the handle's contents changed.
        fn rebuild(&mut self) {
            View::<AppState>::rebuild(
                &self.view,
                &self.view,
                &mut self.widget,
                &mut BuildCtx::new(&mut self.counter),
            );
        }

        fn frame(&mut self, ms: f64, theme: &Theme) -> Recorder {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, self.size, ft_ms(ms))
                .with_theme(theme as &dyn Any);
            self.widget.paint(&mut ctx, &mut rec);
            rec
        }
    }

    #[test]
    fn a_closing_panel_fades_and_zooms_back_out_then_stops_painting() {
        let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::popover()));
        let mut h = PanelHarness::new(style.clone());
        let theme = light();
        // Settle the entrance first, so the exit starts from a full panel.
        h.frame(0.0, &theme);
        h.frame(ENTRANCE_MS as f64 * 2.0, &theme);
        assert!((h.widget.progress() - 1.0).abs() < 1e-9);

        style.borrow_mut().open = false;
        h.rebuild();
        let start = ENTRANCE_MS as f64 * 2.0;
        // The closing edge still paints a whole panel; the ramp only starts
        // moving on the frame after (the controller seeds its clock first).
        let edge = h.frame(start, &theme);
        assert_eq!(edge.rrects.len(), 1, "the panel is still there");

        let mid = h.frame(start + ENTRANCE_MS as f64 / 2.0, &theme);
        assert_eq!(mid.rrects.len(), 1, "and still painting");
        assert!(
            mid.layers[0] > 0.0 && mid.layers[0] < 1.0,
            "at a partial alpha"
        );
        assert_eq!(mid.transforms.len(), 1, "with the zoom-out riding along");
        let later = h.frame(start + ENTRANCE_MS as f64 * 0.75, &theme);
        assert!(later.layers[0] < mid.layers[0], "presence is decreasing");

        let gone = h.frame(start + ENTRANCE_MS as f64 * 2.0, &theme);
        assert!(gone.rrects.is_empty(), "nothing painted at settle");
        assert_eq!(h.widget.progress(), 0.0);
        // And it stays gone.
        assert!(
            h.frame(start + ENTRANCE_MS as f64 * 4.0, &theme)
                .rrects
                .is_empty()
        );
    }

    #[test]
    fn reduce_motion_takes_a_closing_panel_away_on_the_frame_it_closes() {
        let mut reduced = light();
        reduced.motion.reduce_motion = true;
        let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::popover()));
        let mut h = PanelHarness::new(style.clone());
        assert_eq!(h.frame(0.0, &reduced).rrects.len(), 1, "open at once");

        style.borrow_mut().open = false;
        h.rebuild();
        assert!(
            h.frame(16.0, &reduced).rrects.is_empty(),
            "and closed at once — no ramp in either direction"
        );
        assert_eq!(h.widget.progress(), 0.0);
    }

    #[test]
    fn reopening_mid_exit_ramps_back_up_from_the_presence_it_had() {
        let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::popover()));
        let mut h = PanelHarness::new(style.clone());
        let theme = light();
        h.frame(0.0, &theme);
        h.frame(ENTRANCE_MS as f64 * 2.0, &theme);

        let start = ENTRANCE_MS as f64 * 2.0;
        style.borrow_mut().open = false;
        h.rebuild();
        h.frame(start, &theme);
        let half = h.frame(start + ENTRANCE_MS as f64 / 2.0, &theme);
        let interrupted = half.layers[0];
        assert!(interrupted > 0.0 && interrupted < 1.0);

        style.borrow_mut().open = true;
        h.rebuild();
        let resumed = h.frame(start + ENTRANCE_MS as f64 / 2.0, &theme);
        assert_eq!(
            resumed.layers[0], interrupted,
            "the entrance picks up where the exit left off, not at zero"
        );
        let climbing = h.frame(start + ENTRANCE_MS as f64 * 0.75, &theme);
        assert!(climbing.layers[0] > interrupted, "and climbs from there");
        let settled = h.frame(start + ENTRANCE_MS as f64 * 3.0, &theme);
        assert!(settled.layers.is_empty(), "back to a plain composite");
        assert!((h.widget.progress() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_kept_mounted_popover_reports_the_close_at_once_and_paints_it_out() {
        let mut h = Harness::kept_mounted();
        h.click(10.0, 10.0);
        assert!(h.state.open);
        h.paint_at(ENTRANCE_MS as f64 * 2.0);

        // The light dismiss: app state is truthful immediately, on the very
        // event that closed it.
        h.event(pointer(PointerPhase::Down, 380.0, 580.0));
        assert_eq!(h.state.opens, vec![true, false], "reported at once");
        assert!(!h.state.open);
        h.pass();

        let start = h.clock;
        let mid = h.paint_at(start + ENTRANCE_MS as f64 / 2.0);
        assert!(
            mid.rrects
                .iter()
                .any(|(_, _, _, c)| *c == light().scheme().surface_container_high),
            "the panel is still on screen through the ramp"
        );
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);

        let gone = h.paint_at(start + ENTRANCE_MS as f64 * 2.0);
        assert!(
            !gone
                .rrects
                .iter()
                .any(|(_, _, _, c)| *c == light().scheme().surface_container_high),
            "and gone at settle"
        );
    }

    #[test]
    fn a_closing_popover_consumes_nothing_so_the_click_lands_under_it() {
        let mut h = Harness::kept_mounted();
        h.click(10.0, 10.0);
        h.paint_at(ENTRANCE_MS as f64 * 2.0);
        let placed = crate::overlay::place(
            h.anchor.rect(),
            Size::new(
                CONTENT.width + 2.0 * POPOVER_PADDING,
                CONTENT.height + 2.0 * POPOVER_PADDING,
            ),
            Rect::from_origin_size(Point::ORIGIN, WINDOW),
            OverlayPlacement::default(),
        );
        let c = placed.center();

        // A press outside closes it; the panel is now mid-ramp over `c`.
        h.event(pointer(PointerPhase::Down, 380.0, 580.0));
        h.pass();
        assert!(!h.state.open);
        let mid = h.paint_at(h.clock + ENTRANCE_MS as f64 / 2.0);
        assert!(mid.layers[0] > 0.0, "still painting where the press lands");

        let before = h.state.opens.len();
        let outcome = h
            .root
            .event(&mut h.state, &pointer(PointerPhase::Down, c.x, c.y));
        assert!(
            !outcome.handled,
            "a closing panel swallows nothing — the page under it keeps the press"
        );
        assert_eq!(h.state.opens.len(), before, "and it dismisses nothing");
    }

    #[test]
    fn a_pressable_child_keeps_its_own_press_and_the_trigger_stays_quiet() {
        #[derive(Default)]
        struct Presses {
            clicks: u32,
            opens: u32,
        }
        let anchor = OverlayAnchor::new();
        let mut root: RenderRoot<Presses, PopoverTriggerView<Presses>> = RenderRoot::new();
        let mut state = Presses::default();
        let a = anchor.clone();
        let mut logic = move |_s: &mut Presses| {
            popover_trigger(&a, crate::button("Open", |s: &mut Presses| s.clicks += 1))
                .on_open_change(|s: &mut Presses, _| s.opens += 1)
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 18.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 20.0, 18.0));
        assert_eq!(state.clicks, 1, "the button owns the press");
        assert_eq!(state.opens, 0, "so the wrapper reports nothing");
    }
}
