//! Ports beUI's `tooltip` component — `components/motion/tooltip.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `GAP = 8` | [`TOOLTIP_OFFSET`] |
//! | `delay = 120` | [`TOOLTIP_DELAY`] |
//! | `WARM_WINDOW_MS = 300` | [`TOOLTIP_WARM_WINDOW`] |
//! | panel `rounded-lg border border-border bg-background px-2.5 py-1` | [`style::RADIUS_LG`], [`TOOLTIP_PAD_X`], [`TOOLTIP_PAD_Y`] |
//! | label `text-xs font-medium text-foreground` | [`style::TEXT_XS`] through [`crate::text::label_style`] |
//! | `initial {scale: 0.9}`, `exit {scale: 0.94, 0.12s}` | the host's own [`ANCHORED_ENTER_SCALE`](crate::overlay::ANCHORED_ENTER_SCALE) / [`ANCHORED_EXIT`](crate::overlay::ANCHORED_EXIT) |
//! | `transformOrigin` per side | the host's own [`crate::overlay::transform_origin`] |
//! | all four `side`s | [`OverlaySide`] |
//!
//! # The hover decision lives in a widget, not in app state
//!
//! Every other overlay in this catalog is opened by a *click*, so the app can
//! own the flag: a press is an event, an event carries `&mut State`, and the
//! next rebuild mounts the panel. A hover-opened overlay cannot work that way —
//! the open moment is "the pointer has rested on the trigger for
//! [`TOOLTIP_DELAY`]", and a resting pointer sends no events at all. The only
//! per-frame pass a plugin-tier widget gets is `paint`, which carries a clock
//! ([`PaintCtx::frame_time`](frust::authoring::PaintCtx::frame_time)) and the
//! authoritative hover read
//! ([`PaintCtx::is_hovered`](frust::authoring::PaintCtx::is_hovered)) but no
//! application state.
//!
//! So the open flag lives in [`TooltipHover`], a shared non-reactive latch — the
//! same `Rc<Cell<_>>` shape [`OverlayAnchor`] itself uses, read the same way,
//! during the pass that follows the paint which wrote it. [`tooltip_trigger`]
//! latches hover, runs the delay off the frame clock and writes it; [`tooltip`]
//! reads it in its own `View::rebuild` and hands the result to the shared
//! [`crate::overlay::anchored`] host as that host's open flag. A trigger with a
//! pending decision asks for the next frame, so the rebuild that reads the latch
//! always arrives.
//!
//! `on_open_change` is still reported so an app can mirror the state, but
//! **best-effort and one pass late**: the widget can only reach state from an
//! event pass, and the open it is reporting happened during a paint. It is a
//! notification, not the mechanism. [`TooltipTriggerView::open`] takes the
//! decision over entirely instead (upstream's controlled `open`).
//!
//! # A press while the label is up
//!
//! [`crate::overlay::anchored`] consumes every press that lands outside its
//! content — that is its light dismiss, and its own module docs record why the
//! click-through arm upstream's `use-dismiss.ts` defaults to (`"pass-through"`)
//! cannot be built on frust today. A tooltip is the one overlay for which that
//! bites: upstream's panel is `pointer-events-none` and its `useDismiss`
//! excludes the anchor, so clicking the button you are hovering just works.
//!
//! **The port's answer:** the host's dismissal closes the *latch* as well as
//! reporting itself, and marks the trigger suppressed until the pointer leaves
//! it. The first press after a label has appeared is therefore swallowed and
//! closes the label; the second reaches the control underneath; and moving away
//! and back re-arms the tooltip. That is upstream's own tap-toggle shape
//! (`toggleOnTap`) arriving on desktop as well. It is a real degradation, not a
//! design — an input-transparent mode on the anchored host would remove it, and
//! that is a seam change, not a component one.
//!
//! # Degradations against the web original
//!
//! - **One swallowed press per hover session**, above.
//! - **No blur filter.** Upstream's entrance and exit carry `filter: blur(5px)`
//!   / `blur(3px)`; `PaintScene` has no blur primitive, so the staging is the
//!   host's scale + fade only.
//! - **No entrance offset.** `offsetFrom` slides the panel 8px out of the
//!   trigger as it scales. The seam owns the staging and translates nothing, and
//!   its scale about the panel edge nearest the anchor already reads as growing
//!   out of the trigger, so the extra 8px is dropped rather than fought for.
//! - **No flip by default.** Upstream never flips a tooltip to the opposite
//!   side, and neither does this, because the [arrow](TooltipView::arrow)'s tip
//!   is drawn for the side that was *asked for* — a flip the panel cannot
//!   observe would leave it pointing away from the trigger.
//!   [`TooltipView::flip`] turns the host's collision flip back on for a caller
//!   who would rather have the overflow handled; the arrow is dropped when it
//!   does.
//! - **The arrow is an addition.** `tooltip.tsx` draws no tip at all. One is
//!   provided here, and defaulted on, because the catalog's tooltip is expected
//!   to carry one; [`TooltipView::arrow`] switches it off for the literal
//!   upstream silhouette.
//! - **`shadow-lg` becomes the `.glass` chrome shadow** every panel in this
//!   catalog casts (`crate::tokens::glass_scale`), the one recipe the overlay
//!   hosts reserve fade-layer room for.
//! - **Touch is inert.** A finger never hovers, so a touch device never sees a
//!   tooltip — the same story `plugins/shadcn`'s
//!   `shadcn-hover-overlays-touch-inert` limitation records, inherited verbatim.
//!   Upstream's `useTapGesture` tap-to-toggle path is not ported either: it
//!   needs the pointer *type*, which `PointerEvent` does not carry.
//! - **No focus opening.** Upstream shows the label on `onFocus` as well as on
//!   hover. `PaintCtx::has_focus` would answer it, but a trigger here is a
//!   transparent wrapper that claims no focus of its own, and claiming one would
//!   change the keyboard order of whatever it wraps.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    Role, SemanticsCtx, Size, Vec2, View, Widget, any, build_child, erase_callback_arg,
    rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{PanelChrome, paint_panel, resolve_panel};
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAnchor, OverlayPlacement, OverlaySide,
    TOOLTIP_OFFSET, anchored,
};
use crate::press::inside;
use crate::style;
use crate::text::{LabelRun, label_style};

/// How long the pointer rests on a trigger before its label appears
/// (`tooltip.tsx`'s `delay` default).
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(120);

/// How long after one label hides its neighbours open with no delay
/// (`WARM_WINDOW_MS`) — moving along a toolbar feels instant after the first
/// one.
pub const TOOLTIP_WARM_WINDOW: Duration = Duration::from_millis(300);

/// `px-2.5` — the panel's horizontal padding, in logical px.
pub const TOOLTIP_PAD_X: f64 = 10.0;

/// `py-1` — the panel's vertical padding, in logical px.
pub const TOOLTIP_PAD_Y: f64 = 4.0;

/// How far the arrow tip reaches past the panel, in logical px. An addition —
/// upstream draws no tip (see the [module docs](self)).
pub const TOOLTIP_ARROW_SIZE: f64 = 6.0;

/// How wide the arrow's base is, in logical px.
pub const TOOLTIP_ARROW_WIDTH: f64 = 12.0;

thread_local! {
    /// `lastHiddenAt`: when any tooltip on this thread last hid, so the next one
    /// can skip its delay inside [`TOOLTIP_WARM_WINDOW`].
    ///
    /// Module-level upstream, and module-level here — the warm window is
    /// deliberately *shared* between triggers, which is the whole effect.
    static LAST_HIDDEN: Cell<Option<FrameTime>> = const { Cell::new(None) };
}

/// A view-held open-change callback.
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// Whether a tooltip opening at `now` is inside the warm window.
fn warm(now: FrameTime) -> bool {
    LAST_HIDDEN.with(|cell| {
        cell.get()
            .is_some_and(|then| now.saturating_sub(then) < TOOLTIP_WARM_WINDOW)
    })
}

/// Record that a tooltip hid at `now`.
fn mark_hidden(now: FrameTime) {
    LAST_HIDDEN.with(|cell| cell.set(Some(now)));
}

/// The shared hover state of one tooltip: the trigger's window-space rect,
/// whether the label is showing, and whether that trigger is suppressed until
/// the pointer leaves it.
///
/// Clone it — the clone shares the same cells. An app keeps one per tooltip in
/// its `Component::State`, hands a clone to [`tooltip_trigger`] and another to
/// [`tooltip`].
///
/// **Not reactive**, exactly like [`OverlayAnchor`]: writing it wakes no frame.
/// The trigger writes it during a paint that has already asked for the next
/// frame, and the tooltip view reads it in that frame's rebuild.
#[derive(Clone, Debug, Default)]
pub struct TooltipHover {
    anchor: OverlayAnchor,
    open: Rc<Cell<bool>>,
    /// Set by a dismissal or a press, cleared when the pointer leaves the
    /// trigger. See the [module docs](self) on the swallowed press.
    suppressed: Rc<Cell<bool>>,
}

impl TooltipHover {
    /// A fresh latch: closed, unsuppressed, with no trigger rect captured yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the label is showing.
    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// The cell carrying the trigger's window-space rect.
    pub fn anchor(&self) -> &OverlayAnchor {
        &self.anchor
    }

    /// Force the label open or closed — the write a controlled tooltip performs,
    /// also usable by an app driving one itself.
    pub fn set_open(&self, open: bool) {
        self.open.set(open);
    }

    /// Whether the trigger is suppressed until the pointer leaves it.
    pub fn is_suppressed(&self) -> bool {
        self.suppressed.get()
    }

    /// Suppress (or re-arm) the trigger.
    pub fn set_suppressed(&self, suppressed: bool) {
        self.suppressed.set(suppressed);
    }
}

// ---- The panel -------------------------------------------------------------

/// The tooltip's own panel: one line of `text-xs font-medium` on the
/// `background` surface, inside a hairline, with an optional tip.
struct TooltipPanelView {
    label: String,
    side: OverlaySide,
    arrow: bool,
}

/// The retained widget for a tooltip panel.
pub struct TooltipPanelWidget {
    label: LabelRun,
    side: OverlaySide,
    arrow: bool,
    /// The panel's own box inside this widget, excluding the arrow's reach.
    panel: Rect,
}

impl TooltipPanelWidget {
    /// The panel's box inside this widget, excluding the arrow's reach.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    /// How far the arrow reaches past the panel on each axis.
    fn arrow_reach(&self) -> Size {
        if !self.arrow {
            return Size::ZERO;
        }
        if self.side.is_vertical() {
            Size::new(0.0, TOOLTIP_ARROW_SIZE)
        } else {
            Size::new(TOOLTIP_ARROW_SIZE, 0.0)
        }
    }

    /// The arrow's triangle, in this widget's own space: base flush with the
    /// panel edge facing the anchor, apex on the anchor's side of it.
    fn arrow_path(&self) -> BezPath {
        let panel = self.panel;
        let half = TOOLTIP_ARROW_WIDTH / 2.0;
        let centre = panel.center();
        let (base_a, base_b, apex) = match self.side {
            // The panel sits above the anchor, so the tip points down.
            OverlaySide::Top => (
                Point::new(centre.x - half, panel.y1),
                Point::new(centre.x + half, panel.y1),
                Point::new(centre.x, panel.y1 + TOOLTIP_ARROW_SIZE),
            ),
            OverlaySide::Bottom => (
                Point::new(centre.x - half, panel.y0),
                Point::new(centre.x + half, panel.y0),
                Point::new(centre.x, panel.y0 - TOOLTIP_ARROW_SIZE),
            ),
            OverlaySide::Left => (
                Point::new(panel.x1, centre.y - half),
                Point::new(panel.x1, centre.y + half),
                Point::new(panel.x1 + TOOLTIP_ARROW_SIZE, centre.y),
            ),
            OverlaySide::Right => (
                Point::new(panel.x0, centre.y - half),
                Point::new(panel.x0, centre.y + half),
                Point::new(panel.x0 - TOOLTIP_ARROW_SIZE, centre.y),
            ),
        };
        let mut path = BezPath::new();
        path.move_to(base_a);
        path.line_to(apex);
        path.line_to(base_b);
        path.close_path();
        path
    }
}

impl<State: 'static> View<State> for TooltipPanelView {
    type Element = TooltipPanelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TooltipPanelWidget {
        TooltipPanelWidget {
            label: LabelRun::new(self.label.clone()),
            side: self.side,
            arrow: self.arrow,
            panel: Rect::ZERO,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut TooltipPanelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.label.set_content(self.label.clone()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.side != self.side || element.arrow != self.arrow {
            element.side = self.side;
            element.arrow = self.arrow;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, _element: &mut TooltipPanelWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for TooltipPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let text = self.label.layout(ctx, &label_style(style::TEXT_XS));
        let panel = Size::new(
            text.width + TOOLTIP_PAD_X * 2.0,
            text.height + TOOLTIP_PAD_Y * 2.0,
        );
        let reach = self.arrow_reach();
        // The arrow lives inside this widget's own box, on the anchor-facing
        // side, so the host's placement gap is measured from the tip rather than
        // from the panel edge — the one metric the addition moves.
        let origin = match self.side {
            OverlaySide::Bottom => Point::new(0.0, reach.height),
            OverlaySide::Right => Point::new(reach.width, 0.0),
            OverlaySide::Top | OverlaySide::Left => Point::ORIGIN,
        };
        self.panel = Rect::from_origin_size(origin, panel);
        bc.constrain(Size::new(
            panel.width + reach.width,
            panel.height + reach.height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let chrome = tooltip_chrome(Theme::from_paint_ctx(ctx));
        if self.arrow {
            // Painted before the panel so the panel's own surface and hairline
            // cover the triangle's base — the seam the tip would otherwise show.
            scene.fill_path(
                ctx.origin(),
                &self.arrow_path(),
                &Brush::Solid(chrome.surface),
            );
        }
        paint_panel(scene, ctx.origin(), self.panel, style::RADIUS_LG, chrome);
        let text = self.label.size();
        let inner_height = self.panel.height() - TOOLTIP_PAD_Y * 2.0;
        let at = ctx.origin()
            + self.panel.origin().to_vec2()
            + Vec2::new(
                TOOLTIP_PAD_X,
                TOOLTIP_PAD_Y + (inner_height - text.height) / 2.0,
            );
        self.label.paint(at, chrome.ink, scene);
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        // A tooltip is a label, not a control: it activates on nothing, and the
        // host above it owns the dismissal.
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.label.content().to_string();
        ctx.push_node(Role::Tooltip, |node| node.set_label(label.as_str()));
    }
}

/// The tooltip panel's chrome: `bg-background`, not the `popover` surface every
/// other overlay panel in this catalog fills with — `tooltip.tsx` is the one
/// that names `--background` directly.
fn tooltip_chrome(theme: Option<&Theme>) -> PanelChrome {
    let mut chrome = resolve_panel(theme);
    chrome.surface = match theme {
        Some(theme) => theme.scheme().surface,
        None => crate::BEUI_LIGHT.background,
    };
    chrome
}

// ---- The layer -------------------------------------------------------------

/// A declarative beUI tooltip layer. See [`tooltip`].
pub struct TooltipView<State: 'static> {
    label: String,
    hover: TooltipHover,
    side: OverlaySide,
    arrow: bool,
    flip: bool,
    on_open_change: Option<OnOpenChange<State>>,
}

/// Build a tooltip layer showing `label` against the trigger `hover` names, to
/// be mounted **unconditionally** as the top child of a full-area
/// [`frust::Stack`] over the page carrying that trigger.
///
/// The layer is kept mounted and drives itself from the latch — see the [module
/// docs](self) for why the decision lives in the widgets rather than in app
/// state.
pub fn tooltip<State: 'static>(
    hover: &TooltipHover,
    label: impl Into<String>,
) -> TooltipView<State> {
    TooltipView {
        label: label.into(),
        hover: hover.clone(),
        side: OverlaySide::Top,
        arrow: true,
        flip: false,
        on_open_change: None,
    }
}

impl<State: 'static> TooltipView<State> {
    /// Set the side of the trigger the label opens on (default
    /// [`OverlaySide::Top`], `tooltip.tsx`'s own default).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.side = side;
        self
    }

    /// Draw the tip pointing at the trigger (default `true`). An addition — see
    /// the [module docs](self).
    pub fn arrow(mut self, arrow: bool) -> Self {
        self.arrow = arrow;
        self
    }

    /// Turn the host's collision flip on (default `false`, upstream's own
    /// behaviour). The arrow is dropped while it is on, because a flipped panel
    /// cannot tell its tip to turn round.
    pub fn flip(mut self, flip: bool) -> Self {
        self.flip = flip;
        self
    }

    /// Set the dismissal callback: the press the host swallowed, or Escape once
    /// it holds focus, reports `false`. The hover-driven *open* is reported by
    /// [`TooltipTriggerView::on_open_change`] instead.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Some(Rc::new(on_open_change));
        self
    }

    /// The host this view is a thin configuration of, rebuilt every pass so the
    /// latch is re-read.
    fn host(&self) -> AnchoredOverlayView<State> {
        let arrow = self.arrow && !self.flip;
        let placement = OverlayPlacement::on(self.side)
            .offset(TOOLTIP_OFFSET)
            .flip(self.flip);
        let hover = self.hover.clone();
        let reported = self.on_open_change.clone();
        anchored(TooltipPanelView {
            label: self.label.clone(),
            side: self.side,
            arrow,
        })
        .anchor(self.hover.anchor())
        .placement(placement)
        .open(self.hover.is_open())
        .on_dismiss(move |state| {
            // The dismissal has to reach the latch, not just the app: the latch
            // is what the next rebuild reads, so a host that dismissed while the
            // pointer still rested on the trigger would be reopened on the spot
            // and swallow the next press too. Suppressing until the pointer
            // leaves is the whole of the mitigation the module docs describe.
            hover.set_open(false);
            hover.set_suppressed(true);
            if let Some(reported) = &reported {
                reported(state, false);
            }
        })
    }
}

impl<State: 'static> View<State> for TooltipView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.host(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // The latch is read *here*, on the rebuild the trigger's paint asked
        // for, which is what turns a paint-time hover decision into the host's
        // own open flag.
        View::rebuild(&self.host(), &prev.host(), element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.host(), element, ctx);
    }
}

// ---- The trigger -----------------------------------------------------------

/// Wrap `child` as a tooltip trigger: it captures its own window-space rect into
/// `hover`, latches the pointer resting on it, and opens the label after
/// [`TooltipTriggerView::delay`].
///
/// Transparent in every other respect — it lays out, paints, routes and
/// publishes the semantics of `child` unchanged, and claims nothing but hover.
pub fn tooltip_trigger<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    child: V,
) -> TooltipTriggerView<State> {
    TooltipTriggerView {
        child: any(child),
        hover: hover.clone(),
        delay: TOOLTIP_DELAY,
        open: None,
        on_open_change: Rc::new(|_, _| {}),
    }
}

/// A declarative tooltip trigger. See [`tooltip_trigger`].
pub struct TooltipTriggerView<State: 'static> {
    child: AnyView<State>,
    hover: TooltipHover,
    delay: Duration,
    open: Option<bool>,
    on_open_change: OnOpenChange<State>,
}

impl<State: 'static> TooltipTriggerView<State> {
    /// Set how long the pointer must rest before the label appears (default
    /// [`TOOLTIP_DELAY`]). [`Duration::ZERO`] is upstream's own
    /// `delayDuration={0}` shape.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Take the open decision over entirely — upstream's controlled `open`.
    /// `None` (the default) leaves it to the hover clock.
    pub fn open(mut self, open: Option<bool>) -> Self {
        self.open = open;
        self
    }

    /// Set the open-change callback — a best-effort mirror of the latch,
    /// reported from the first event pass after the paint that changed it.
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
    /// The controlled override, when the caller took the decision over.
    controlled: Option<bool>,
    /// When the current hover started; `None` while the pointer is elsewhere.
    since: Option<FrameTime>,
    /// The latch value last reported through `on_open_change`.
    reported: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl TooltipTriggerWidget {
    /// When the current hover started — `None` while the pointer is elsewhere.
    pub fn hovered_since(&self) -> Option<FrameTime> {
        self.since
    }

    /// The delay this hover actually owes: none inside the warm window.
    fn effective_delay(&self, now: FrameTime) -> Duration {
        if warm(now) {
            Duration::ZERO
        } else {
            self.delay
        }
    }
}

impl<State: 'static> View<State> for TooltipTriggerView<State> {
    type Element = TooltipTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipTriggerWidget {
        TooltipTriggerWidget {
            child: build_child(&self.child, ctx),
            hover: self.hover.clone(),
            delay: self.delay,
            controlled: self.open,
            since: None,
            reported: self.hover.is_open(),
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
        // `PaintCtx::origin` is absolute window space — the one read that
        // answers "where is this trigger on screen", which is what the host
        // needs and what `getBoundingClientRect()` answers upstream.
        self.hover
            .anchor()
            .set(Rect::from_origin_size(ctx.origin(), ctx.size()));

        let now = ctx.frame_time();
        if let Some(open) = self.controlled {
            // A controlled tooltip runs no clock at all: the caller's value is
            // the latch, every frame.
            self.since = None;
            self.hover.set_open(open);
        } else if ctx.is_hovered() {
            let since = *self.since.get_or_insert(now);
            if !self.hover.is_open() && !self.hover.is_suppressed() {
                if now.saturating_sub(since) >= self.effective_delay(now) {
                    self.hover.set_open(true);
                }
                // Either the delay has just elapsed (and the rebuild that reads
                // the latch has to happen) or it has not (and the frame that
                // reaches it does). A resting pointer sends nothing, so this is
                // the only thing keeping the clock running.
                ctx.request_frame();
            }
        } else {
            self.since = None;
            // Leaving re-arms a trigger a dismissal or a press suppressed —
            // upstream's tap toggle resets on the same boundary.
            self.hover.set_suppressed(false);
            if self.hover.is_open() {
                self.hover.set_open(false);
                mark_hidden(now);
                ctx.request_frame();
            }
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The latch mirror, drained before anything else so a change made during
        // a paint is reported on the first pass that can carry it.
        let open = self.hover.is_open();
        if open != self.reported {
            self.reported = open;
            (self.on_open_change)(ctx, open);
        }
        // The child owns every gesture; this region claims hover *after* routing
        // (the container-claims-after-routing rule), and only so that a trigger
        // wrapping something inert still reads as hovered.
        let result = route_event_single(&mut self.child, ctx, event);
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Move if inside(p.position, ctx.size()) => ctx.claim_hover(),
                PointerPhase::Down if self.controlled.is_none() => {
                    // A press with no label up is the tap path: it must not open
                    // one behind the control it just activated.
                    self.since = None;
                    self.hover.set_suppressed(true);
                }
                _ => {}
            }
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, escape, ft_ms, light, pointer};
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct App {
        opens: Vec<bool>,
    }

    /// The trigger's own box. It sits at the window origin, so a pointer event
    /// at `(10, 10)` lands on it and one at `(400, 400)` does not.
    const TRIGGER: Size = Size::new(120.0, 40.0);

    /// The mount an app uses: the trigger in the page, the layer as the top
    /// child of a full-area [`frust::Stack`], permanently.
    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        hover: TooltipHover,
        controlled: Option<bool>,
        side: OverlaySide,
        arrow: bool,
    }

    impl Harness {
        fn new() -> Self {
            LAST_HIDDEN.with(|cell| cell.set(None));
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                tcx: TextContext::new(),
                hover: TooltipHover::new(),
                controlled: None,
                side: OverlaySide::Top,
                arrow: true,
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        /// One whole frame at `ms`: rebuild, layout, paint.
        fn frame(&mut self, ms: f64) {
            let hover = self.hover.clone();
            let controlled = self.controlled;
            let side = self.side;
            let arrow = self.arrow;
            let mut logic = move |_s: &mut App| {
                frust::Stack(vec![
                    any(tooltip_trigger(
                        &hover,
                        SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                    )
                    .open(controlled)
                    .on_open_change(|s: &mut App, open| s.opens.push(open))),
                    any(tooltip(&hover, "Add to library")
                        .side(side)
                        .arrow(arrow)
                        .on_open_change(|s: &mut App, open| s.opens.push(open))),
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

        /// Hover the trigger and hold there long enough to open the label,
        /// then take the extra frame whose *rebuild* reads the latch the
        /// previous frame's paint wrote — the one-frame lag the module docs
        /// describe.
        fn open(&mut self, at: f64) {
            let delay = TOOLTIP_DELAY.as_secs_f64() * 1000.0;
            self.event(pointer(PointerPhase::Move, 10.0, 10.0));
            self.frame(at);
            self.frame(at + delay);
            self.frame(at + delay + 16.0);
        }
    }

    // ---- The hover clock --------------------------------------------------

    #[test]
    fn a_resting_pointer_opens_the_label_only_after_the_delay() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        assert!(!h.hover.is_open(), "the delay has not elapsed");
        h.frame(TOOLTIP_DELAY.as_secs_f64() * 1000.0 - 1.0);
        assert!(!h.hover.is_open(), "still short of it");
        h.frame(TOOLTIP_DELAY.as_secs_f64() * 1000.0);
        assert!(
            h.hover.is_open(),
            "the delay elapsed with no further event in sight"
        );
    }

    #[test]
    fn leaving_the_trigger_hides_the_label_with_no_delay_of_its_own() {
        let mut h = Harness::new();
        h.open(0.0);
        assert!(h.hover.is_open());
        h.event(pointer(PointerPhase::Move, 400.0, 400.0));
        h.frame(200.0);
        assert!(!h.hover.is_open());
    }

    #[test]
    fn the_warm_window_lets_the_next_label_skip_its_delay() {
        let mut h = Harness::new();
        h.open(0.0);
        h.event(pointer(PointerPhase::Move, 400.0, 400.0));
        h.frame(200.0);
        assert!(
            !h.hover.is_open(),
            "hidden, and the warm window is now open"
        );
        // Back inside 300ms: the very next hovered frame opens it.
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(300.0);
        assert!(h.hover.is_open(), "no delay inside the warm window");
    }

    #[test]
    fn the_latch_is_mirrored_to_app_state_on_the_next_event_pass() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY.as_secs_f64() * 1000.0);
        assert!(h.state.opens.is_empty(), "a paint reaches no state");
        h.event(pointer(PointerPhase::Move, 11.0, 10.0));
        assert_eq!(h.state.opens, vec![true], "the next pass flushes it");
        h.event(pointer(PointerPhase::Move, 12.0, 10.0));
        assert_eq!(h.state.opens, vec![true], "reported once per change");
    }

    #[test]
    fn a_controlled_open_pins_the_latch_and_bypasses_the_clock() {
        let mut h = Harness::new();
        h.controlled = Some(true);
        h.frame(0.0);
        assert!(h.hover.is_open(), "no hover, no delay, still open");
        h.controlled = Some(false);
        h.frame(16.0);
        assert!(!h.hover.is_open());
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(1_000.0);
        assert!(
            !h.hover.is_open(),
            "hovering does not move a controlled one"
        );
    }

    // ---- The swallowed press ----------------------------------------------

    #[test]
    fn the_press_that_dismisses_the_label_suppresses_it_until_the_pointer_leaves() {
        let mut h = Harness::new();
        h.open(0.0);
        // The host swallows this one and closes the latch (the documented
        // degradation), so the label does not immediately reopen under a pointer
        // that never moved.
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!h.hover.is_open());
        assert!(h.hover.is_suppressed());
        h.frame(500.0);
        h.frame(1_000.0);
        assert!(!h.hover.is_open(), "a suppressed trigger never reopens");

        // Leaving re-arms it.
        h.event(pointer(PointerPhase::Move, 400.0, 400.0));
        h.frame(1_100.0);
        assert!(!h.hover.is_suppressed());
        h.open(1_200.0);
        assert!(h.hover.is_open());
    }

    #[test]
    fn a_press_on_a_closed_trigger_reaches_the_child_and_opens_nothing() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        h.frame(0.0);
        h.frame(1_000.0);
        assert!(
            !h.hover.is_open(),
            "a tap must not raise a label behind what it activated"
        );
    }

    #[test]
    fn escape_closes_the_label_once_the_host_holds_focus() {
        let mut h = Harness::new();
        h.open(0.0);
        // A press claims focus for the host (and is swallowed); Escape then
        // reaches it.
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        h.event(escape());
        assert!(!h.hover.is_open());
    }

    // ---- The panel --------------------------------------------------------

    #[test]
    fn the_layer_paints_nothing_while_closed_and_a_panel_while_open() {
        let mut h = Harness::new();
        assert!(
            h.paint_at(0.0).rrects.is_empty(),
            "no panel while the latch is closed"
        );
        h.open(0.0);
        let open = h.paint_at(2_000.0);
        let theme = light();
        assert!(
            open.rrects
                .iter()
                .any(|(_, _, r, c)| *r == style::RADIUS_LG && *c == theme.scheme().surface),
            "the panel fills `bg-background` at `rounded-lg`"
        );
        assert!(!open.inks.is_empty(), "and carries its label");
    }

    #[test]
    fn the_panel_sits_a_tooltip_gap_off_the_trigger_with_the_tip_between_them() {
        let mut h = Harness::new();
        h.side = OverlaySide::Bottom;
        h.open(0.0);
        let rec = h.paint_at(2_000.0);
        let (origin, _, _, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, r, _)| *r == style::RADIUS_LG)
            .expect("the panel");
        // The whole widget starts `GAP` below the trigger's own bottom edge, and
        // the tip occupies the first `TOOLTIP_ARROW_SIZE` of it — the one metric
        // the addition moves (see the module docs).
        assert_eq!(
            origin.y,
            TRIGGER.height + TOOLTIP_OFFSET + TOOLTIP_ARROW_SIZE
        );
    }

    #[test]
    fn the_tip_is_dropped_when_the_flip_is_asked_for() {
        // The arrow is drawn for the side that was asked for, so a host allowed
        // to flip gets no tip at all.
        let mut h = Harness::new();
        h.open(0.0);
        let with_tip = h.paint_at(2_000.0);
        let tipped = with_tip.paths;

        let mut flipped = Harness::new();
        flipped.arrow = false;
        flipped.open(0.0);
        let bare = flipped.paint_at(2_000.0).paths;
        assert!(
            tipped > bare,
            "the tip is one filled path the bare one lacks"
        );
    }

    #[test]
    fn the_panel_hugs_one_line_of_text_inside_its_own_padding() {
        let mut h = Harness::new();
        h.side = OverlaySide::Bottom;
        h.open(0.0);
        let rec = h.paint_at(2_000.0);
        let (_, size, _, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, r, _)| *r == style::RADIUS_LG)
            .expect("the panel");
        assert!(
            size.height > TOOLTIP_PAD_Y * 2.0 && size.width > TOOLTIP_PAD_X * 2.0,
            "the panel is its text plus its padding: {size:?}"
        );
    }
}
