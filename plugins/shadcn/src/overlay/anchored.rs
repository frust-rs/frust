//! The anchored (non-modal) overlay host: a full-area layer that positions its
//! content relative to a trigger's rect, the way Radix's `Popper` positions a
//! portalled popover against its anchor.
//!
//! Source behavior: `@radix-ui/react-popper` as consumed by
//! `apps/v4/registry/new-york-v4/ui/popover.tsx`, `tooltip.tsx`,
//! `dropdown-menu.tsx`, `hover-card.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — `side`
//! (default `bottom`), `align` (default `center`), `sideOffset` (`4` in every
//! shadcn wrapper but `tooltip`'s `0`), and `avoidCollisions` (default on:
//! flip to the opposite side when the preferred one does not fit, then shift
//! back inside the boundary).
//!
//! # The three pieces
//!
//! 1. [`place`] — the placement math, a pure function over
//!    `(anchor, content size, area, `[`OverlayPlacement`]`)`. It is public and
//!    unit-tested on its own; the host below is the only in-crate caller.
//! 2. [`OverlayAnchor`] + [`anchor`] — the trigger side. The anchor rect is
//!    **window space**, and the only place a widget learns its window-space
//!    position is [`PaintCtx::origin`](frust::authoring::PaintCtx::origin), which
//!    is absolute; so [`anchor`] wraps the trigger view in a transparent pod
//!    that writes `Rect::from_origin_size(ctx.origin(), ctx.size())` into a
//!    shared cell on every paint. The host reads the same cell.
//! 3. [`anchored`] — the host itself: a full-area widget holding one content
//!    child, positioned by [`place`], light-dismissing on a press outside it.
//!
//! # Mounting, and what an exit animation costs
//!
//! There are two ways to drive an anchored overlay, and they differ only in what
//! happens on the way *out*:
//!
//! * **Mount-on-open** (the simplest): the app mounts the host while its own flag
//!   is set and drops it when the flag clears. The overlay appears with its
//!   entrance ramp and disappears instantly, because the widget it would have
//!   ramped in is gone by the next frame.
//! * **Kept-mounted** (what an exit animation needs): the app mounts the host
//!   *unconditionally* and hands the flag down through
//!   [`AnchoredOverlayView::open`]. A closed host lays its content out, paints
//!   nothing once the exit ramp settles, and is completely input-transparent —
//!   so it costs one layout of the content per frame and nothing else.
//!
//! The framework has no seam for keeping a conditionally-mounted view alive past
//! the rebuild that unmounts it, so an exit ramp is only possible for a widget
//! that **stays mounted with `open == false`**. A rebuild that unmounts the host
//! mid-ramp simply truncates the exit; nothing breaks, the panel just vanishes.
//!
//! A closed host claims no focus, dismisses on nothing, consumes no press and
//! publishes no semantics. Its placement math runs regardless, so
//! [`AnchoredOverlayWidget::content_rect`] stays truthful for the whole ramp.
//!
//! # Coordinate spaces
//!
//! The anchor is captured in absolute window coordinates; the host places
//! content in its own local space. The host learns its own absolute origin from
//! its paint pass (the same `PaintCtx::origin` read) and subtracts it, so the
//! two agree even when the host is not itself at the window origin — with a
//! one-frame lag on the pass where the host first paints or moves, which the
//! paint arm corrects by requesting a relayout. In the supported mounts (a
//! transparent navigator page, or the top child of a full-area
//! [`frust::Stack`]) the host *is* at the window origin, so the correction is
//! the identity and the lag is unobservable.
//!
//! # Dismissal
//!
//! - **Light dismiss**: a `Down` outside the placed content fires `on_dismiss`
//!   and is **consumed**, so it never reaches the app below (Radix's own
//!   modal-dismiss shape; a click-through variant is not offered in v1). A
//!   `Down` *inside* the content routes to the content and, if nothing there
//!   takes it, is swallowed — pressing a panel's own background never dismisses.
//! - **Escape**: dismisses once the host holds focus. The host claims focus on
//!   every `Down` it sees (the `frust_material::dialog` precedent), so — exactly
//!   as for every modal widget in the workspace — a caller must complete one
//!   pointer interaction with the overlay before Escape does anything. There is
//!   still no auto-focus-on-appear hook in the framework.
//! - **Move/Scroll pass through**: only `Down` (and a focus-routed `Escape`) is
//!   consumed outside the content. A `Move` outside is reported
//!   [`EventResult::Ignored`] and the host claims no hover of its own, so a
//!   hover-driven overlay (tooltip, hover-card) can still see its trigger's
//!   hover end underneath the host.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, SemanticsCtx, Size, View, Widget, any, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child, visit_children,
};

use super::finite_or_zero;

/// The side of the anchor an overlay opens on — Radix's `side` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlaySide {
    /// Above the anchor.
    Top,
    /// To the trailing side of the anchor.
    Right,
    /// Below the anchor — Radix's default.
    #[default]
    Bottom,
    /// To the leading side of the anchor.
    Left,
}

impl OverlaySide {
    /// The side a collision flip lands on.
    pub fn opposite(self) -> Self {
        match self {
            OverlaySide::Top => OverlaySide::Bottom,
            OverlaySide::Bottom => OverlaySide::Top,
            OverlaySide::Left => OverlaySide::Right,
            OverlaySide::Right => OverlaySide::Left,
        }
    }

    /// Whether this side stacks the overlay vertically (`Top`/`Bottom`).
    pub fn is_vertical(self) -> bool {
        matches!(self, OverlaySide::Top | OverlaySide::Bottom)
    }
}

/// How the overlay lines up with the anchor on the cross axis — Radix's `align`
/// prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayAlign {
    /// Leading edges flush (top edges for a left/right side, left edges for a
    /// top/bottom one).
    Start,
    /// Centers flush — Radix's default.
    #[default]
    Center,
    /// Trailing edges flush.
    End,
}

/// A resolved placement request: which side, how it lines up, how far off the
/// anchor it sits, and whether it may flip or shift to stay inside the area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayPlacement {
    /// The preferred side (`side`).
    pub side: OverlaySide,
    /// Cross-axis alignment (`align`).
    pub align: OverlayAlign,
    /// Gap between the anchor and the overlay, in logical px (`sideOffset`).
    pub offset: f64,
    /// Flip to [`OverlaySide::opposite`] when the preferred side does not fit
    /// and the opposite one does (`avoidCollisions`).
    pub flip: bool,
    /// Shift the placed rect back inside the area when it overflows
    /// (`avoidCollisions`' second half; Radix's `collisionPadding` is `0` here,
    /// its own default).
    pub clamp: bool,
}

/// shadcn's `sideOffset` on every anchored wrapper but `tooltip` (which passes
/// `0`), in logical px.
pub const SIDE_OFFSET: f64 = 4.0;

impl Default for OverlayPlacement {
    fn default() -> Self {
        OverlayPlacement {
            side: OverlaySide::default(),
            align: OverlayAlign::default(),
            offset: SIDE_OFFSET,
            flip: true,
            clamp: true,
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
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.align = align;
        self
    }

    /// Set the anchor gap, in logical px.
    pub fn offset(mut self, offset: f64) -> Self {
        self.offset = offset;
        self
    }

    /// Enable or disable the collision flip.
    pub fn flip(mut self, flip: bool) -> Self {
        self.flip = flip;
        self
    }

    /// Enable or disable the shift-back-inside clamp.
    pub fn clamp(mut self, clamp: bool) -> Self {
        self.clamp = clamp;
        self
    }
}

/// Whether a `content`-sized overlay fits on `side` of `anchor` inside `area`.
fn fits(side: OverlaySide, anchor: Rect, content: Size, area: Rect, offset: f64) -> bool {
    match side {
        OverlaySide::Top => anchor.y0 - offset - content.height >= area.y0,
        OverlaySide::Bottom => anchor.y1 + offset + content.height <= area.y1,
        OverlaySide::Left => anchor.x0 - offset - content.width >= area.x0,
        OverlaySide::Right => anchor.x1 + offset + content.width <= area.x1,
    }
}

/// The cross-axis start coordinate for `align`, given the anchor's own span
/// `[a0, a1]` and the overlay's `extent` along that axis.
fn align_start(align: OverlayAlign, a0: f64, a1: f64, extent: f64) -> f64 {
    match align {
        OverlayAlign::Start => a0,
        OverlayAlign::Center => (a0 + a1) / 2.0 - extent / 2.0,
        OverlayAlign::End => a1 - extent,
    }
}

/// Shift `rect` back inside `area`, keeping its size.
///
/// The start edge wins when the overlay is larger than the area (`min` before
/// `max`): a too-wide panel hangs off the trailing edge rather than the leading
/// one, which is where its content starts.
fn clamp_into(rect: Rect, area: Rect) -> Rect {
    let x = rect.x0.min(area.x1 - rect.width()).max(area.x0);
    let y = rect.y0.min(area.y1 - rect.height()).max(area.y0);
    Rect::from_origin_size(Point::new(x, y), rect.size())
}

/// Place a `content`-sized overlay against `anchor` inside `area`.
///
/// All three rects are in one coordinate space (the host's own; see the [module
/// docs](self)). The returned rect is the overlay's placed bounds:
///
/// 1. pick the side — the preferred one, or its opposite when `flip` is on, the
///    preferred one does not fit and the opposite does;
/// 2. offset off that edge of the anchor by `offset`, lined up on the cross axis
///    per `align`;
/// 3. shift back inside `area` when `clamp` is on.
///
/// Pure and total: it allocates nothing, reads no context, and is defined for a
/// degenerate anchor (a zero-size rect places against that point) and for
/// content larger than the area (which pins to the area's *start* edge, where
/// the content begins, rather than its trailing one).
pub fn place(anchor: Rect, content: Size, area: Rect, placement: OverlayPlacement) -> Rect {
    let mut side = placement.side;
    if placement.flip
        && !fits(side, anchor, content, area, placement.offset)
        && fits(side.opposite(), anchor, content, area, placement.offset)
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
        clamp_into(rect, area)
    } else {
        rect
    }
}

/// A shared cell holding a trigger's window-space rect: written by [`anchor`]
/// on every paint of the wrapped trigger, read by [`anchored`] on every layout.
///
/// Clone it — the clone shares the same cell. An app keeps one per anchored
/// overlay in its `Component::State`, hands a clone to the trigger and another
/// to the host.
///
/// **Not reactive.** Writing a rect wakes no frame; it is read during the layout
/// pass the same frame anything that moved the trigger already scheduled, and
/// the host's paint arm requests a relayout when it sees the rect change under
/// it. An overlay whose trigger moves with nothing else repainting is therefore
/// one frame behind, never permanently stale.
#[derive(Clone, Debug, Default)]
pub struct OverlayAnchor(Rc<std::cell::Cell<Rect>>);

impl OverlayAnchor {
    /// A fresh anchor, holding the degenerate rect at the window origin until a
    /// trigger paints (see [`OverlayAnchor::rect`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// The captured trigger rect, in window space.
    ///
    /// Before the wrapped trigger's first paint this is `Rect::ZERO` — a
    /// zero-size anchor at the window origin, which places an overlay at the
    /// top-left. In practice a trigger always paints before it can be pressed,
    /// so an overlay opened by that press already has a real rect.
    pub fn rect(&self) -> Rect {
        self.0.get()
    }

    /// Overwrite the captured rect — for an app anchoring against something it
    /// positions itself (a caret, a canvas hit point) rather than a widget.
    pub fn set(&self, rect: Rect) {
        self.0.set(rect);
    }
}

/// Wrap `child` so its window-space rect is captured into `anchor` on every
/// paint — the trigger half of an anchored overlay.
///
/// Transparent in every other respect: it lays out, paints, routes events to and
/// publishes the semantics of `child` unchanged, and paints nothing of its own.
pub fn anchor<State: 'static, V: View<State>>(
    anchor: &OverlayAnchor,
    child: V,
) -> OverlayAnchorView<State> {
    OverlayAnchorView {
        child: any(child),
        anchor: anchor.clone(),
    }
}

/// A declarative anchor-capturing wrapper. See [`anchor`].
pub struct OverlayAnchorView<State: 'static> {
    child: AnyView<State>,
    anchor: OverlayAnchor,
}

/// The retained widget for an [`OverlayAnchorView`].
pub struct OverlayAnchorWidget {
    child: ChildPod,
    anchor: OverlayAnchor,
}

impl<State: 'static> View<State> for OverlayAnchorView<State> {
    type Element = OverlayAnchorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayAnchorWidget {
        OverlayAnchorWidget {
            child: build_child(&self.child, ctx),
            anchor: self.anchor.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayAnchorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.anchor = self.anchor.clone();
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut OverlayAnchorWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for OverlayAnchorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::origin` is absolute window space — the one read that can
        // answer "where is this trigger on screen", which is what an anchored
        // overlay needs.
        self.anchor
            .set(Rect::from_origin_size(ctx.origin(), ctx.size()));
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

/// A view-held, typed dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// Build an anchored-overlay host showing `content`. See the [module docs](self)
/// for the mounting, coordinate-space and dismissal contracts.
///
/// Chain [`AnchoredOverlayView::anchor`] to point it at a trigger,
/// [`AnchoredOverlayView::placement`] to pick the side/align/offset, and
/// [`AnchoredOverlayView::on_dismiss`] to handle a light dismiss or Escape.
pub fn anchored<State: 'static, V: View<State>>(content: V) -> AnchoredOverlayView<State> {
    AnchoredOverlayView {
        content: any(content),
        anchor: OverlayAnchor::new(),
        placement: OverlayPlacement::default(),
        open: true,
        on_dismiss: None,
    }
}

/// A declarative anchored-overlay host. See [`anchored`].
pub struct AnchoredOverlayView<State: 'static> {
    content: AnyView<State>,
    anchor: OverlayAnchor,
    placement: OverlayPlacement,
    open: bool,
    on_dismiss: Option<OnDismiss<State>>,
}

impl<State: 'static> AnchoredOverlayView<State> {
    /// Anchor the overlay to the rect `anchor` carries (see [`OverlayAnchor`]).
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = anchor.clone();
        self
    }

    /// Set the placement (side, align, offset, flip/clamp).
    pub fn placement(mut self, placement: OverlayPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Tell a kept-mounted host whether it is open (see the [module
    /// docs](self)). The default is `true`, which is the mount-on-open contract:
    /// a mounted host is an open one.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the dismiss callback: a press outside the placed content, or Escape
    /// once the host holds focus.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

/// The retained widget for an [`AnchoredOverlayView`].
pub struct AnchoredOverlayWidget {
    content: ChildPod,
    anchor: OverlayAnchor,
    placement: OverlayPlacement,
    /// Whether the overlay is open. `false` makes the host inert without
    /// unmounting it — the kept-mounted pattern the module docs describe.
    open: bool,
    on_dismiss: Option<ErasedCallback>,
    /// The placed content rect, in this host's own coordinate space.
    rect: Rect,
    /// The anchor rect the last layout placed against (window space) — compared
    /// at paint so a trigger that moved forces a relayout.
    anchor_used: Rect,
    /// This host's own absolute origin, learned at paint (see the module docs'
    /// coordinate-space note).
    host_origin: Point,
}

impl AnchoredOverlayWidget {
    /// The placed content rect in the host's own coordinate space — what the
    /// light-dismiss hit test compares against.
    ///
    /// Placement runs whether the host is open or not, so this stays truthful
    /// while a kept-mounted overlay plays its exit ramp.
    pub fn content_rect(&self) -> Rect {
        self.rect
    }

    /// Whether the host is open (see [`AnchoredOverlayView::open`]).
    pub fn is_open(&self) -> bool {
        self.open
    }
}

impl<State: 'static> View<State> for AnchoredOverlayView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        AnchoredOverlayWidget {
            content: build_child(&self.content, ctx),
            anchor: self.anchor.clone(),
            placement: self.placement,
            open: self.open,
            on_dismiss: self.on_dismiss.as_ref().map(erase_callback),
            rect: Rect::ZERO,
            anchor_used: Rect::ZERO,
            host_origin: Point::ORIGIN,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.placement != self.placement {
            element.placement = self.placement;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            // The paint is what starts the content's exit (or entrance) ramp.
            flags |= ChangeFlags::PAINT;
        }
        element.anchor = self.anchor.clone();
        // Closures aren't comparable, so the dismiss adapter is reinstalled
        // unconditionally — cheap, and what every interactive widget does.
        element.on_dismiss = self.on_dismiss.as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for AnchoredOverlayWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        let content = self.content.layout_child(ctx, &BoxConstraints::loose(area));
        self.anchor_used = self.anchor.rect();
        // Window space → this host's own space (the identity in the supported
        // mounts; see the module docs).
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
        // Self-correction, the paint-time mirror of the layout above: this is
        // the only pass that knows the host's absolute origin, and the anchor
        // cell can move under a layout that already ran.
        if ctx.origin() != self.host_origin || self.anchor.rect() != self.anchor_used {
            self.host_origin = ctx.origin();
            ctx.request_layout();
        }
        // The host has no chrome of its own — no scrim, no panel. Everything
        // visible is the content's.
        self.content.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A closed (or closing) kept-mounted host is input-transparent: it
        // claims no focus, dismisses on nothing and swallows no press, so the
        // click that closed it — and every one after — reaches the page below
        // while the content plays its exit ramp. A broadcast still reaches the
        // content, which is what keeps its pods live.
        if !self.open {
            if event.is_broadcast() {
                self.content.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // Claim focus on every `Down`, before the routing below — the
        // `frust_material::dialog` opt-in that makes Escape reachable at all,
        // and claimed even when the content consumes the press (a control
        // inside the panel claiming focus of its own simply wins the descent;
        // both pods sit on the same chain). Re-claiming while already focused
        // is a no-op — a claim-once guard would drop the root's focus session
        // on the second press.
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            ctx.request_focus();
        }
        // Content first, always: a broadcast reaches it unconsumed, a focused
        // child takes the key events, and a press inside it is its own. The host
        // claims nothing before this (the claim-ordering rule).
        if route_event_single(&mut self.content, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if let InputEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Escape)
                && let Some(on_dismiss) = self.on_dismiss.as_mut()
            {
                on_dismiss(ctx);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                // Deliberately button-agnostic, unlike every press machine in
                // the catalog: a light dismiss is not a press. The browser this
                // ports from closes an open popover/menu on a right-click
                // outside it exactly as on a left-click, and the host captures
                // nothing here — it only fires the app's dismiss callback — so
                // the primary-only press rule has nothing to protect.
                if !self.rect.contains(p.position)
                    && let Some(on_dismiss) = self.on_dismiss.as_mut()
                {
                    on_dismiss(ctx);
                }
                // Consumed either way: dismissing swallows the press, and a
                // press on the content's own background is the panel's.
                EventResult::Handled
            }
            // Everything else passes through, so a trigger under the host keeps
            // seeing its own hover/scroll (the module docs' pass-through note).
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The host contributes no node of its own — the content (a menu, a
        // tooltip) carries the role worth reporting. A closed host reports
        // nothing: a panel on its way out is not there to be read.
        if self.open {
            self.content.semantics_child(ctx);
        }
    }

    visit_children!(content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{
        Color, KeyEvent, Modifiers, PointerButton, PointerEvent, text::TextContext,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 600.0);
    const WINDOW: Size = Size::new(400.0, 600.0);

    fn anchor_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::from_origin_size(Point::new(x, y), Size::new(w, h))
    }

    // ---- placement math ---------------------------------------------------

    #[test]
    fn the_default_placement_is_shadcns_bottom_center_at_four_px() {
        let p = OverlayPlacement::default();
        assert_eq!(p.side, OverlaySide::Bottom);
        assert_eq!(p.align, OverlayAlign::Center);
        assert_eq!(p.offset, SIDE_OFFSET);
        assert!(p.flip && p.clamp);
    }

    #[test]
    fn each_side_offsets_off_its_own_edge() {
        let a = anchor_rect(100.0, 200.0, 80.0, 40.0);
        let content = Size::new(120.0, 60.0);
        // The raw side placement, with the collision passes off: `left` would
        // otherwise flip (a 120px panel does not fit in the 100px to the
        // anchor's left), which the flip test below covers on its own.
        let raw = |side| OverlayPlacement::on(side).flip(false).clamp(false);
        let bottom = place(a, content, AREA, raw(OverlaySide::Bottom));
        assert_eq!(bottom.y0, a.y1 + SIDE_OFFSET);
        let top = place(a, content, AREA, raw(OverlaySide::Top));
        assert_eq!(top.y1, a.y0 - SIDE_OFFSET);
        let right = place(a, content, AREA, raw(OverlaySide::Right));
        assert_eq!(right.x0, a.x1 + SIDE_OFFSET);
        let left = place(a, content, AREA, raw(OverlaySide::Left));
        assert_eq!(left.x1, a.x0 - SIDE_OFFSET);
        // Sizes are never altered by placement.
        for r in [bottom, top, right, left] {
            assert_eq!(r.size(), content);
        }
    }

    #[test]
    fn align_lines_up_leading_center_or_trailing_edges() {
        let a = anchor_rect(100.0, 200.0, 80.0, 40.0);
        let content = Size::new(120.0, 60.0);
        let at = |align| {
            place(
                a,
                content,
                AREA,
                OverlayPlacement::on(OverlaySide::Bottom).align(align),
            )
        };
        assert_eq!(at(OverlayAlign::Start).x0, a.x0);
        assert_eq!(at(OverlayAlign::End).x1, a.x1);
        assert_eq!(at(OverlayAlign::Center).center().x, a.center().x);

        // The same three on a horizontal side act on the vertical axis.
        let at = |align| {
            place(
                a,
                content,
                AREA,
                OverlayPlacement::on(OverlaySide::Right).align(align),
            )
        };
        assert_eq!(at(OverlayAlign::Start).y0, a.y0);
        assert_eq!(at(OverlayAlign::End).y1, a.y1);
        assert_eq!(at(OverlayAlign::Center).center().y, a.center().y);
    }

    #[test]
    fn a_side_that_does_not_fit_flips_to_the_opposite_one() {
        // An anchor near the bottom edge: `bottom` overflows, `top` fits.
        let a = anchor_rect(100.0, 560.0, 80.0, 20.0);
        let content = Size::new(120.0, 100.0);
        let flipped = place(a, content, AREA, OverlayPlacement::default());
        assert_eq!(flipped.y1, a.y0 - SIDE_OFFSET, "flipped above the anchor");

        // With the flip disabled it stays below and only the clamp moves it.
        let pinned = place(a, content, AREA, OverlayPlacement::default().flip(false));
        assert_eq!(pinned.y1, AREA.y1, "clamped, not flipped");

        // Neither side fits: the preferred one is kept (and clamped).
        let tall = Size::new(120.0, 590.0);
        let kept = place(a, tall, AREA, OverlayPlacement::default());
        assert_eq!(kept.y1, AREA.y1);
    }

    #[test]
    fn clamping_shifts_the_rect_back_inside_and_can_be_turned_off() {
        // An anchor at the right edge, centre-aligned: the panel overflows.
        let a = anchor_rect(380.0, 100.0, 20.0, 20.0);
        let content = Size::new(200.0, 50.0);
        let clamped = place(a, content, AREA, OverlayPlacement::default());
        assert_eq!(clamped.x1, AREA.x1);
        assert_eq!(clamped.y0, a.y1 + SIDE_OFFSET, "only the cross axis moved");

        let free = place(a, content, AREA, OverlayPlacement::default().clamp(false));
        assert!(free.x1 > AREA.x1, "unclamped placement may overflow");

        // Content wider than the area pins to the leading edge (the start-edge
        // rule), not the trailing one.
        let huge = Size::new(600.0, 50.0);
        let pinned = place(a, huge, AREA, OverlayPlacement::default());
        assert_eq!(pinned.x0, AREA.x0);
    }

    #[test]
    fn an_uncaptured_anchor_places_at_the_area_origin() {
        let content = Size::new(80.0, 40.0);
        let rect = place(Rect::ZERO, content, AREA, OverlayPlacement::default());
        assert_eq!(rect.x0, AREA.x0, "clamped in from the centred overhang");
        assert_eq!(rect.y0, SIDE_OFFSET);
    }

    // ---- the host ---------------------------------------------------------

    #[derive(Default)]
    struct AppState {
        dismissed: u32,
        pressed: u32,
    }

    /// A fixed-size leaf that reports every press into the app state — enough to
    /// prove a press inside the content reached it rather than the host.
    struct Panel(Size);

    /// The retained half of [`Panel`].
    struct PanelWidget(Size);

    impl View<AppState> for Panel {
        type Element = PanelWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PanelWidget {
            PanelWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut PanelWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for PanelWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            if p.phase == PointerPhase::Down {
                ctx.state_mut::<AppState>().pressed += 1;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    const CONTENT: Size = Size::new(120.0, 60.0);
    const ANCHOR: Rect = Rect::new(100.0, 200.0, 180.0, 240.0);

    /// The host mounted the way an app mounts it: as the top child of a
    /// full-area [`frust::Stack`] (see the module docs). The `Stack` is
    /// load-bearing for the Escape tests — it is the container that focus-routes
    /// a key event to whichever child holds the focus path, which is what the
    /// host's own focus claim buys.
    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
    }

    impl Harness {
        fn new() -> Self {
            let anchor = OverlayAnchor::new();
            anchor.set(ANCHOR);
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor,
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            let anchor = self.anchor.clone();
            let mut logic = move |_s: &mut AppState| {
                frust::Stack(vec![any(anchored(Panel(CONTENT))
                    .anchor(&anchor)
                    .on_dismiss(|s: &mut AppState| s.dismissed += 1))])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventResult {
            let outcome = self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
            if outcome.handled {
                EventResult::Handled
            } else {
                EventResult::Ignored
            }
        }

        fn escape(&mut self) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Escape),
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }
    }

    #[test]
    fn the_host_fills_its_area_and_places_the_content_against_the_anchor() {
        let mut h = Harness::new();
        let rect = place(ANCHOR, CONTENT, AREA, OverlayPlacement::default());
        // Bottom/center of the anchor, `SIDE_OFFSET` below it.
        assert_eq!(rect.y0, ANCHOR.y1 + SIDE_OFFSET);
        assert_eq!(rect.center().x, ANCHOR.center().x);
        // Nothing painted by the host itself; the content lands at the placement.
        let mut rec = Recorder::default();
        h.root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(rec.rects, vec![(rect.origin(), CONTENT)]);
    }

    #[test]
    fn an_unbounded_height_collapses_the_host_area_and_pins_the_content_to_its_top() {
        // The documented mounts (a navigator page, a full-area `Stack`) are
        // bounded; a host put inside a scroll view is not, and this is what
        // that costs — pinned, not fixed here: the coercion cannot invent an
        // extent nobody offered, so the fix belongs at the mount site.
        let cell = OverlayAnchor::new();
        cell.set(ANCHOR);
        let view = anchored(Panel(CONTENT)).anchor(&cell);
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
            w.rect.y0, 0.0,
            "the panel clamps to the top of a zero-height area, wherever its \
             anchor sits"
        );

        // The same host under the contract's own constraints places normally.
        let bounded = w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        assert_eq!(bounded, WINDOW);
        assert_eq!(w.rect.y0, ANCHOR.y1 + SIDE_OFFSET);
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    #[test]
    fn a_press_outside_the_content_dismisses_and_is_swallowed() {
        let mut h = Harness::new();
        assert_eq!(
            h.pointer(PointerPhase::Down, 5.0, 5.0),
            EventResult::Handled,
            "the light dismiss consumes the press"
        );
        assert_eq!(h.state.dismissed, 1);
        assert_eq!(h.state.pressed, 0, "the content never saw it");
    }

    #[test]
    fn a_press_inside_the_content_routes_to_it_and_never_dismisses() {
        let mut h = Harness::new();
        let rect = place(ANCHOR, CONTENT, AREA, OverlayPlacement::default());
        let c = rect.center();
        assert_eq!(
            h.pointer(PointerPhase::Down, c.x, c.y),
            EventResult::Handled
        );
        assert_eq!(h.state.pressed, 1);
        assert_eq!(h.state.dismissed, 0);
    }

    #[test]
    fn moves_outside_the_content_pass_through_untouched() {
        let mut h = Harness::new();
        assert_eq!(
            h.pointer(PointerPhase::Move, 5.0, 5.0),
            EventResult::Ignored,
            "a hover-driven trigger under the host keeps its own moves"
        );
        assert_eq!(h.state.dismissed, 0);
    }

    #[test]
    fn escape_dismisses_once_a_press_has_focused_the_host() {
        let mut h = Harness::new();
        h.escape();
        assert_eq!(
            h.state.dismissed, 0,
            "no focus yet, so Escape has no chain to travel"
        );
        // A press on the content focuses the host without dismissing it.
        let rect = place(ANCHOR, CONTENT, AREA, OverlayPlacement::default());
        let c = rect.center();
        h.pointer(PointerPhase::Down, c.x, c.y);
        h.pointer(PointerPhase::Up, c.x, c.y);
        h.escape();
        assert_eq!(h.state.dismissed, 1);
    }

    #[test]
    fn a_moved_anchor_is_re_placed_on_the_next_layout() {
        let mut h = Harness::new();
        h.anchor.set(anchor_rect(20.0, 20.0, 40.0, 20.0));
        h.pass();
        let mut rec = Recorder::default();
        h.root.paint(&mut rec, FrameTime::ZERO);
        let expected = place(h.anchor.rect(), CONTENT, AREA, OverlayPlacement::default());
        assert_eq!(rec.rects[0].0, expected.origin());
    }

    // ---- the trigger-side capture -----------------------------------------

    #[test]
    fn the_anchor_wrapper_captures_the_triggers_window_rect_at_paint() {
        let cell = OverlayAnchor::new();
        assert_eq!(cell.rect(), Rect::ZERO, "nothing captured before a paint");

        let mut root: RenderRoot<AppState, frust::PaddingView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let captured = cell.clone();
        // Offset by a padding so the captured rect proves it is absolute, not
        // parent-relative.
        let mut logic = move |_s: &mut AppState| {
            frust::Padding(
                frust::EdgeInsets::symmetric(24.0, 12.0),
                anchor(&captured, Panel(CONTENT)),
            )
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), FrameTime::ZERO);

        assert_eq!(
            cell.rect(),
            Rect::from_origin_size(Point::new(24.0, 12.0), CONTENT)
        );
    }
}
