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
//! | `initial {scale: 0.9}`, `exit {scale: 0.94, 0.12s}` | the panel's own [`ANCHORED_ENTER_SCALE`](crate::overlay::ANCHORED_ENTER_SCALE) / [`ANCHORED_EXIT`](crate::overlay::ANCHORED_EXIT), reused from the anchored host this port no longer mounts through |
//! | `transformOrigin` per side | the panel's own use of [`crate::overlay::transform_origin`] |
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
//! latches hover, runs the delay off the frame clock and writes it; [`tooltip`]'s
//! own panel ([`TooltipPanelWidget`]) reads it directly on every paint, since
//! the framework overlay portal it now floats through carries no open flag of
//! its own to hand it to. A trigger with a pending decision asks for the next
//! frame, so the paint that reads the latch always arrives.
//!
//! `on_open_change` is still reported so an app can mirror the state, but
//! **best-effort and one pass late**: the widget can only reach state from an
//! event pass, and the open it is reporting happened during a paint. It is a
//! notification, not the mechanism. [`TooltipTriggerView::open`] takes the
//! decision over entirely instead (upstream's controlled `open`).
//!
//! # Riding the framework portal
//!
//! Every other anchored panel in this catalog floats through
//! [`crate::overlay::anchored`], whose light dismiss consumes every press that
//! lands outside its content by design. Mounted permanently under a hover
//! trigger — which every tooltip is — that swallowed the very first press made
//! anywhere while the label was up, whatever it happened to land on, and closed
//! the label doing it. That degradation is retired: [`tooltip`] no longer
//! mounts through [`crate::overlay::anchored`] at all.
//!
//! Its panel is registered through [`frust::authoring::OverlaySlot`] instead —
//! the framework's own overlay-portal seam — declared
//! [`frust::authoring::OverlayBand::Tooltip`] +
//! [`frust::authoring::OverlayInput::Transparent`]. The root's hit-test
//! pre-pass skips a `Transparent` entry outright, so every press reaches the
//! page exactly as if the tooltip were not there — whether it lands on the
//! trigger, on the panel's own background (a tooltip is a label, not a
//! control, so it has no press of its own to answer either way), or anywhere
//! else. `crate::overlay::anchored` itself is unmodified: every click-opened
//! overlay in the catalog keeps using it.
//!
//! [`frust::authoring::overlay_portal`]'s own declarative wrapper anchors only
//! to its own child's bounds, which does not fit a hover-opened panel whose
//! trigger and label stay two independently-mounted views — exactly
//! `plugins/shadcn`'s own tooltip's reasoning for the same choice. So this
//! port drives [`frust::authoring::OverlaySlot`] directly, anchored to the
//! rect [`TooltipHover`] already carries, rather than using that wrapper.
//!
//! [`TooltipPanelWidget`] is therefore the pod actually registered with the
//! root — the only widget that runs during the root's own separate overlay
//! paint pass — which is why it is also what now stages the entrance/exit
//! scale [`crate::overlay::anchored`] used to own (see the mapping table
//! above): the framework portal has no ramp of its own, so the panel plays
//! [`ANCHORED_ENTER_SCALE`](crate::overlay::ANCHORED_ENTER_SCALE) /
//! [`ANCHORED_EXIT`](crate::overlay::ANCHORED_EXIT) itself, reusing the very
//! ramp `crate::overlay::anchored` is built from — the animation is kept, not
//! dropped, just relocated. [`TooltipLayerWidget`] (this view's own retained
//! widget) is the thin owner left: it decides only *whether* to keep
//! registering that pod each paint — open, or within
//! [`ANCHORED_EXIT`](crate::overlay::ANCHORED_EXIT) of the latch having
//! closed, so the exit is never truncated — and mirrors the latch's *close*
//! back through [`TooltipView::on_open_change`]. **Before this port**, that
//! callback fired `false` only when `crate::overlay::anchored`'s light
//! dismiss closed the panel — an outside press, or Escape once it held focus
//! — never on an ordinary hover-out. Neither mechanism exists any more, so
//! [`TooltipLayerWidget`] now reports `false` on every open→closed transition
//! of the latch instead, which includes the hover-out that ends every
//! session: the exact transition [`TooltipTriggerView::on_open_change`]
//! already reports. A caller installing both callbacks therefore receives
//! `false` **twice** for one close; [`TooltipView::on_open_change`] is kept
//! only so a caller already holding one from before this port still compiles,
//! not because it reports anything the trigger's own callback does not.
//!
//! # Degradations against the web original
//!
//! - **No accessibility node while the label is up, for now.** The panel now
//!   floats through [`frust::authoring::OverlaySlot`], whose own docs list
//!   "the pod contributes no semantics" among what the shared portal seam
//!   does not do in v1 — the root never walks a registered pod's
//!   `semantics`, so [`TooltipPanelWidget`]'s node is unreachable today; a
//!   screen reader learns nothing where the tooltip's text would be, only
//!   the trigger's own node. The wiring itself is kept live rather than
//!   deleted — a deliberate choice, matching `plugins/shadcn`'s own ported
//!   tooltip — so the node resumes reaching an assistive technology the day
//!   that v1 restriction lifts, with nothing here needing re-authoring.
//!   `crate::overlay::anchored`'s panels never had this gap, since they
//!   painted through the ordinary tree walk; it is a cost specific to this
//!   port's switch to the framework portal, not a `tooltip.tsx` degradation.
//! - **No Escape dismissal.** `crate::overlay::anchored` claimed focus on a
//!   press and closed on Escape; [`frust::authoring::OverlayInput::Transparent`]
//!   claims no focus at all (nor does the trigger), so Escape now has nothing
//!   to reach. A hover session ends only when the pointer leaves the trigger —
//!   the same shape `plugins/shadcn`'s own tooltip was ported to.
//! - **No blur filter.** Upstream's entrance and exit carry `filter: blur(5px)`
//!   / `blur(3px)`; `PaintScene` has no blur primitive, so the staging is the
//!   panel's own scale + fade only.
//! - **No entrance offset.** `offsetFrom` slides the panel 8px out of the
//!   trigger as it scales. The panel's own local ramp translates nothing, and
//!   its scale about the edge nearest the anchor already reads as growing out
//!   of the trigger, so the extra 8px is dropped rather than fought for.
//! - **No flip by default.** Upstream never flips a tooltip to the opposite
//!   side, and neither does this, because the [arrow](TooltipView::arrow)'s tip
//!   is drawn for the side that was *asked for* — a flip the panel cannot
//!   observe would leave it pointing away from the trigger.
//!   [`TooltipView::flip`] turns the framework portal's own collision flip
//!   back on for a caller who would rather have the overflow handled; the
//!   arrow is dropped when it does.
//! - **The arrow is an addition.** `tooltip.tsx` draws no tip at all. One is
//!   provided here, and defaulted on, because the catalog's tooltip is expected
//!   to carry one; [`TooltipView::arrow`] switches it off for the literal
//!   upstream silhouette.
//! - **`shadow-lg` becomes the `.glass` chrome shadow** every panel in this
//!   catalog casts (`crate::tokens::glass_scale`). `crate::overlay::anchored`'s
//!   own paint reserves extra room around its scale layer specifically
//!   because a layer sized to the resting rect alone clips the shadow at
//!   *every* partial alpha, not only while closing — that host pushes the
//!   wider layer whenever `alpha < 1.0`, entrance included. The panel's own
//!   local ramp does not reserve that room, so a sliver of the shadow may
//!   clip near the panel's edges on *both* the entrance and the exit — and
//!   for longer on the entrance: [`ANCHORED_EXIT`](crate::overlay::ANCHORED_EXIT)
//!   is a fixed 120ms, while the entrance rides `Ramp::spring(SPRING_PANEL)`
//!   (mass 0.5 / stiffness 420 / damping 40, over-damped), whose analytic
//!   settle estimate runs several times longer. The same simplification
//!   `plugins/shadcn`'s ported tooltip makes, for the same
//!   panel-owns-its-ramp reason.
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
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, OutsideTap,
    OverlayAnchor as PortalAnchor, OverlayBand, OverlayInput, OverlayPlacement as PortalPlacement,
    OverlaySide as PortalSide, OverlaySlot, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    SemanticsCtx, Size, Vec2, View, Widget, any, build_child, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child, visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{PanelChrome, paint_panel, resolve_panel};
use crate::motion::{Presence, PresencePhase, Ramp};
use crate::overlay::{
    ANCHORED_ENTER_SCALE, ANCHORED_EXIT, ANCHORED_EXIT_SCALE, OverlayAlign, OverlayAnchor,
    OverlaySide, TOOLTIP_OFFSET, finite_or_zero, transform_origin,
};
use crate::press::inside;
use crate::style;
use crate::text::{LabelRun, ThemeTextType, label_style};
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};

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

/// The type-scale role the label takes its family from at layout — a small
/// caption.
const LABEL_ROLE: ThemeTextType = ThemeTextType::LabelSmall;

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
    /// Set by every press that lands on the trigger — [`TooltipTriggerWidget::event`]'s
    /// `PointerPhase::Down` arm writes it unconditionally, whether or not the
    /// label is already open — and cleared when the pointer leaves the
    /// trigger. Only ever *consulted* while closed
    /// ([`TooltipTriggerWidget::paint`]'s guard reads it inside `!is_open()`),
    /// so a press while the label is already showing sets it with no visible
    /// effect: the very next hover-out clears it again before the label could
    /// reopen. No longer written by a dismissal: the framework portal the
    /// panel floats through consumes no press (see the [module docs](self)),
    /// so the trigger's own tap path above is the only writer left.
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
    hover: TooltipHover,
}

/// The retained widget for a tooltip panel — the pod actually registered with
/// the root's overlay mechanism, so this is also where the entrance/exit
/// scale plays (see the [module docs](self)'s "Riding the framework portal").
pub struct TooltipPanelWidget {
    label: LabelRun,
    side: OverlaySide,
    arrow: bool,
    /// The panel's own box inside this widget, excluding the arrow's reach.
    panel: Rect,
    hover: TooltipHover,
    /// Whether the latch was open as of the last paint — an edge starts a
    /// ramp, in whichever direction the edge went.
    was_open: bool,
    presence: Presence,
    /// The `reduce_motion` value `presence` was last built for; `None` until
    /// the first paint resolves a theme.
    reduced: Option<bool>,
}

impl TooltipPanelWidget {
    /// The panel's box inside this widget, excluding the arrow's reach.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    /// A fresh presence driver on the same ramps `crate::overlay::anchored`
    /// builds its own from — entrance on a spring, exit eased over
    /// [`ANCHORED_EXIT`].
    fn fresh_presence() -> Presence {
        Presence::new(
            Ramp::spring(SPRING_PANEL),
            Ramp::eased(ANCHORED_EXIT, EASE_OUT),
        )
    }

    /// Rebuild the presence driver when `reduce_motion` flips.
    ///
    /// A simpler policy than
    /// `crate::overlay::anchored::AnchoredOverlayWidget::sync_motion`'s: it
    /// restarts the active ramp from the driver's own beginning rather than
    /// preserving progress, since a caller flipping `reduce_motion` mid-fade
    /// on a tooltip is not a case this port optimizes for.
    fn sync_motion(&mut self, reduce: bool) {
        if self.reduced == Some(reduce) {
            return;
        }
        self.reduced = Some(reduce);
        let base = Self::fresh_presence();
        self.presence = if reduce { base.collapsed() } else { base };
        self.presence.set_open(self.was_open);
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
            hover: self.hover.clone(),
            was_open: false,
            presence: TooltipPanelWidget::fresh_presence(),
            reduced: None,
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
        element.hover = self.hover.clone();
        flags
    }

    fn teardown(&self, _element: &mut TooltipPanelWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for TooltipPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let text = self
            .label
            .layout_themed(ctx, &label_style(style::TEXT_XS), LABEL_ROLE);
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
        // The framework overlay portal owns no ramp of its own (see the
        // [module docs](self)), so this pod plays the same entrance/exit
        // scale `crate::overlay::anchored` used to stage from the outside.
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|theme| theme.motion.reduce_motion);
        self.sync_motion(reduce);

        let open = self.hover.is_open();
        if open != self.was_open {
            self.was_open = open;
            self.presence.set_open(open);
        }
        let progress = self.presence.advance(ctx.frame_time());
        if self.presence.is_animating() {
            ctx.request_frame();
        }
        if !self.presence.is_visible() {
            // Settled closed: `TooltipLayerWidget` already stops registering
            // this pod once its own grace window lapses, so this only ever
            // guards the one frame in between.
            return;
        }

        let from = if self.presence.phase() == PresencePhase::Exiting {
            ANCHORED_EXIT_SCALE
        } else {
            ANCHORED_ENTER_SCALE
        };
        let scale = from + (1.0 - from) * progress;
        let alpha = progress.clamp(0.0, 1.0) as f32;
        // `ctx.origin()` is this pod's own placed rect origin in window
        // space — the root paints a registered pod directly, at the rect it
        // registered — so this needs no extra translation the way the old
        // host's own locally-placed rect did.
        let rect = Rect::from_origin_size(ctx.origin(), ctx.size());
        let pivot = transform_origin(self.side, OverlayAlign::Center, rect);

        let layer_pushed = alpha < 1.0;
        if layer_pushed {
            scene.push_layer(rect.origin(), rect.size(), alpha);
        }
        scene.push_transform(
            Affine::translate(pivot.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-pivot.to_vec2()),
        );

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

        scene.pop_transform();
        if layer_pushed {
            scene.pop_layer();
        }
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        // A tooltip is a label, not a control: it activates on nothing. Never
        // reached in practice — a `Transparent` pod is never dispatched an
        // ordinary or broadcast event (see `frust_widgets::overlay`'s
        // `OverlaySlot::event_ambient`) — but `Widget` still requires it.
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Unreachable today: the root never walks a pod registered through
        // the framework overlay portal for semantics in v1 (see the [module
        // docs](self)'s Degradations list). Kept live rather than deleted, so
        // the node returns for free the day that restriction lifts, rather
        // than needing to be re-authored from scratch — the same choice
        // `plugins/shadcn`'s own ported tooltip makes for its panel.
        if self.hover.is_open() {
            let label = self.label.content().to_string();
            ctx.push_node(Role::Tooltip, |node| node.set_label(label.as_str()));
        }
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

/// Build a tooltip layer showing `label` against the trigger `hover` names.
///
/// Mount it **unconditionally**, alongside [`tooltip_trigger`] under any live
/// ancestor — it no longer has to be a full-area [`frust::Stack`]'s topmost
/// child: the root's own [`frust::authoring::OverlayBand`] ordering paints
/// its panel above the whole main tree regardless of where in the tree this
/// layer sits, and [`frust::authoring::OverlayInput::Transparent`] means it
/// costs no input priority either. The layer is kept mounted and drives
/// itself from the latch — see the [module docs](self) for why the decision
/// lives in the widgets rather than in app state.
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

    /// Turn the framework portal's collision flip on (default `false`,
    /// upstream's own behaviour). The arrow is dropped while it is on,
    /// because a flipped panel cannot tell its tip to turn round.
    pub fn flip(mut self, flip: bool) -> Self {
        self.flip = flip;
        self
    }

    /// Set a callback reporting the panel's own close.
    ///
    /// **Before this port**, this fired `false` only when
    /// `crate::overlay::anchored`'s light dismiss closed the panel — an
    /// outside press, or Escape once it held focus — and never on an
    /// ordinary hover-out. **Now** that host is gone, so this fires `false`
    /// on every open→closed transition of the [`TooltipHover`] latch instead,
    /// including the hover-out that ends every session: the exact transition
    /// [`TooltipTriggerView::on_open_change`] already reports (see the
    /// [module docs](self)). Installing both callbacks therefore delivers
    /// `false` **twice** for one close. It never reported the hover-driven
    /// *open* either before this port or after;
    /// [`TooltipTriggerView::on_open_change`] is the one callback worth
    /// installing today for a caller that only needs one.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Some(Rc::new(on_open_change));
        self
    }

    /// The panel view this layer floats, rebuilt every pass so a changed
    /// label, side or arrow flag reaches the pod and the latch is re-read.
    fn content(&self) -> AnyView<State> {
        any(TooltipPanelView {
            label: self.label.clone(),
            side: self.side,
            arrow: self.arrow && !self.flip,
            hover: self.hover.clone(),
        })
    }

    /// The placement the framework portal's slot is configured with.
    fn slot_placement(&self) -> PortalPlacement {
        let side = match self.side {
            OverlaySide::Top => PortalSide::Top,
            OverlaySide::Right => PortalSide::Right,
            OverlaySide::Bottom => PortalSide::Bottom,
            OverlaySide::Left => PortalSide::Left,
        };
        PortalPlacement::on(side)
            .offset(TOOLTIP_OFFSET)
            .flip(self.flip)
    }
}

impl<State: 'static> View<State> for TooltipView<State> {
    type Element = TooltipLayerWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipLayerWidget<State> {
        let mut slot = OverlaySlot::new();
        slot.set_band(OverlayBand::Tooltip);
        slot.set_input(OverlayInput::Transparent);
        slot.set_outside_tap(OutsideTap::Ignore);
        slot.set_placement(self.slot_placement());
        let content = self.content();
        slot.rebuild(None, Some(&content), ctx);
        TooltipLayerWidget {
            hover: self.hover.clone(),
            slot,
            was_open: false,
            closing_since: None,
            reported_open: self.hover.is_open(),
            on_open_change: self.on_open_change.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipLayerWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.slot.set_placement(self.slot_placement());
        element.hover = self.hover.clone();
        element.on_open_change = self.on_open_change.clone();
        let prev_content = prev.content();
        let next_content = self.content();
        element
            .slot
            .rebuild(Some(&prev_content), Some(&next_content), ctx)
            | ChangeFlags::PAINT
    }

    fn teardown(&self, element: &mut TooltipLayerWidget<State>, ctx: &mut BuildCtx<'_>) {
        let content = self.content();
        element.slot.rebuild(Some(&content), None, ctx);
    }
}

/// The retained widget for a [`TooltipView`]: owns a
/// [`frust::authoring::OverlaySlot`], anchored to the rect [`TooltipHover`]
/// carries, and decides only *whether* to keep registering it each paint —
/// the ramp itself runs in the registered pod ([`TooltipPanelWidget`]), which
/// is the only widget that actually runs during the root's own separate
/// overlay paint pass (see the [module docs](self)).
pub struct TooltipLayerWidget<State: 'static> {
    hover: TooltipHover,
    slot: OverlaySlot<State>,
    /// Whether the latch was open as of the last paint.
    was_open: bool,
    /// When the latch last closed, if the exit ramp might still be running.
    closing_since: Option<FrameTime>,
    /// The latch value [`TooltipView::on_open_change`] last reported. Only
    /// ever drives a `false` report — see the [module docs](self) on why the
    /// callback is a mirror of [`TooltipTriggerView::on_open_change`] now.
    reported_open: bool,
    on_open_change: Option<OnOpenChange<State>>,
}

impl<State: 'static> Widget for TooltipLayerWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The floated pod is sized against the window
        // (`frust_widgets::overlay::OverlaySlot::layout`), not against `bc` —
        // it escapes this widget's box entirely. Filling the available box
        // below, not `Size::ZERO`, keeps this owner reachable by ordinary
        // routing wherever it sits, matching `crate::overlay::anchored`'s own
        // full-area shape.
        let max = bc.max();
        self.slot.layout(ctx);
        bc.constrain(Size::new(
            finite_or_zero(max.width),
            finite_or_zero(max.height),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        let open = self.hover.is_open();
        if open {
            self.was_open = true;
            self.closing_since = None;
        } else if self.was_open {
            self.was_open = false;
            self.closing_since = Some(now);
        }
        // Whether to keep registering the pod at all: open, or still inside
        // the exit ramp it plays on its own (see [`TooltipPanelWidget::paint`]).
        // Matching `ANCHORED_EXIT` here is what keeps the two in step.
        let reduce_motion = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let grace = if reduce_motion {
            Duration::ZERO
        } else {
            ANCHORED_EXIT
        };
        let showing =
            open || matches!(self.closing_since, Some(since) if now.saturating_sub(since) < grace);
        if !showing {
            return;
        }
        // Window space → this owner's own local space, the translation every
        // `OverlaySlot::set_anchor(OverlayAnchor::Rect(_))` caller makes.
        let local = self.hover.anchor().rect() - ctx.origin().to_vec2();
        self.slot.set_anchor(PortalAnchor::Rect(local));
        self.slot.paint(ctx, Size::ZERO);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let open = self.hover.is_open();
        if open != self.reported_open {
            self.reported_open = open;
            if !open && let Some(on_open_change) = self.on_open_change.clone() {
                on_open_change(ctx.state_mut::<State>(), false);
            }
        }
        // A `Transparent` tooltip pod is never handed an ordinary or
        // broadcast event by the slot (see `OverlaySlot::event_ambient`'s own
        // docs), so this is reached only for completeness with the
        // interactive shapes the same mechanism also serves.
        if let Some(result) = self.slot.event_ambient(ctx, event) {
            return result;
        }
        EventResult::Ignored
    }

    fn semantics(&self, _ctx: &mut SemanticsCtx) {
        // The owner contributes no node of its own, and the registered pod
        // contributes none either in v1 — see the [module docs](self)'s
        // Degradations list.
    }

    visit_children!();
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
    use crate::components::popover::tests::{Recorder, WINDOW, ft_ms, light, pointer};
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct App {
        opens: Vec<bool>,
        /// Set by [`PageButtonWidget`] — the proof consumer for "a press
        /// outside the open tooltip reaches the widget beneath it," not
        /// merely "nobody handled it".
        button_pressed: bool,
    }

    /// A page widget filling whatever area it is given, recording whether it
    /// was pressed. Mounted *beneath* the trigger and the tooltip layer, so a
    /// press that used to be swallowed by `crate::overlay::anchored`'s light
    /// dismiss now has somewhere to land.
    struct PageButton;

    struct PageButtonWidget;

    impl View<App> for PageButton {
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
                ctx.state_mut::<App>().button_pressed = true;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }

        fn semantics(&self, _ctx: &mut SemanticsCtx) {}

        visit_children!();
    }

    /// The trigger's own box. It sits at the window origin, so a pointer event
    /// at `(10, 10)` lands on it and one at `(400, 400)` does not.
    const TRIGGER: Size = Size::new(120.0, 40.0);

    /// The mount an app uses: a page filling the whole area, the trigger on
    /// it, and the tooltip layer — no longer required to be a particular
    /// child of anything (see the [module docs](super)).
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
                frust::stack()
                    .child(PageButton)
                    .child(
                        tooltip_trigger(
                            &hover,
                            SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                        )
                        .open(controlled)
                        .on_open_change(|s: &mut App, open| s.opens.push(open)),
                    )
                    .child(
                        tooltip(&hover, "Add to library")
                            .side(side)
                            .arrow(arrow)
                            .on_open_change(|s: &mut App, open| s.opens.push(open)),
                    )
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
    fn a_close_is_reported_through_both_on_open_change_callbacks() {
        // `TooltipView::on_open_change` no longer has a dismissal to fire
        // from, so it mirrors the same close `TooltipTriggerView::on_open_change`
        // already reports (see the module docs) — a caller installing both
        // receives `false` twice for the one close.
        let mut h = Harness::new();
        h.open(0.0);
        h.event(pointer(PointerPhase::Move, 11.0, 10.0));
        assert_eq!(h.state.opens, vec![true], "the open, reported once");

        h.event(pointer(PointerPhase::Move, 400.0, 400.0));
        h.frame(1_000.0);
        assert!(!h.hover.is_open(), "the paint above already closed it");

        // The next event pass flushes both callbacks' own edge-triggered
        // reports — the layer's mirror runs first (it sits on top), then the
        // trigger's.
        h.event(pointer(PointerPhase::Move, 13.0, 10.0));
        assert_eq!(
            h.state.opens,
            vec![true, false, false],
            "one close, reported by the layer and the trigger each"
        );
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

    // ---- Presses pass through ----------------------------------------------

    #[test]
    fn a_press_outside_the_open_tooltip_reaches_the_widget_beneath_it() {
        let mut h = Harness::new();
        h.open(0.0);
        assert!(h.hover.is_open());
        // Far from both the trigger and the placed panel — exactly the press
        // `crate::overlay::anchored`'s light dismiss used to swallow while a
        // label was up (see the module docs).
        h.event(pointer(PointerPhase::Down, 300.0, 300.0));
        assert!(
            h.state.button_pressed,
            "the page beneath the open tooltip saw the press"
        );
        // Still open: an ordinary press elsewhere is not a dismissal any more.
        assert!(h.hover.is_open());
    }

    #[test]
    fn a_press_on_the_trigger_itself_still_reaches_the_page_while_the_label_is_up() {
        let mut h = Harness::new();
        h.open(0.0);
        // The trigger's own box, not the placed panel — the tooltip no longer
        // has any press of its own to answer, so this reaches straight
        // through to the trigger's tap-path bookkeeping and, since the
        // trigger's child is not itself a control here, past it as well.
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(
            h.state.button_pressed,
            "a press on the trigger's own box is not swallowed either"
        );
    }

    #[test]
    fn a_press_on_the_placed_panel_itself_reaches_the_widget_beneath_it() {
        // The load-bearing guard for `TooltipLayerWidget`'s registration:
        // this is the one press that only a *transparent* overlay lets
        // through, since it lands on the panel's own registered rect rather
        // than merely near it. Flipping `TooltipView::build`'s
        // `slot.set_input(OverlayInput::Transparent)` call to
        // `OverlayInput::Interactive` makes this test fail — the root's
        // overlay pre-pass then claims the press for the tooltip's own pod
        // instead of letting it fall through.
        let mut h = Harness::new();
        h.open(0.0);
        let rec = h.paint_at(2_000.0);
        let (origin, size, _, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, r, _)| *r == style::RADIUS_LG)
            .expect("the panel");
        let center_x = origin.x + size.width / 2.0;
        let center_y = origin.y + size.height / 2.0;
        h.event(pointer(PointerPhase::Down, center_x, center_y));
        assert!(
            h.state.button_pressed,
            "a press on the panel's own placed rect still reaches the page beneath it"
        );
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

    // ---- Typeface: the label follows the live theme ------------------------

    use crate::text::typeface_probe::{
        Face, Probe, assert_all, assert_control, assert_follows_a_live_family_swap_on,
    };

    /// A trigger held open by its controlled flag, so the label floats up
    /// through the framework portal without a hover.
    fn open_probe() -> Probe<frust::StackView<()>, impl FnMut(&mut ()) -> frust::StackView<()>> {
        let hover = TooltipHover::new();
        let logic = move |_: &mut ()| {
            frust::stack()
                .child(
                    tooltip_trigger(&hover, SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)))
                        .open(Some(true)),
                )
                .child(tooltip::<()>(&hover, "Add to library"))
        };
        let mut probe = Probe::new(logic, WINDOW, crate::theme());
        probe.frame();
        probe
    }

    #[test]
    fn the_label_paints_in_geist_under_the_beui_theme() {
        assert_control("the tooltip label", WINDOW);
        assert_all(
            "the tooltip label",
            "under the beUI theme",
            &open_probe().frame(),
            Face::Geist,
        );
    }

    #[test]
    fn the_label_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap_on("the tooltip label", &mut open_probe());
    }
}
