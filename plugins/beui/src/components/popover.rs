//! Ports beUI's `popover` component — the registry slug's two sources in one
//! widget: `components/motion/popover.tsx` and
//! `components/motion/popover-morph.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `sideOffset` default `14` | [`crate::overlay::SIDE_OFFSET`] |
//! | `panelRadius` default `16` | [`POPOVER_RADIUS`] |
//! | content `p-4` | [`POPOVER_PADDING`] |
//! | `max-w-[min(92vw,20rem)]` | [`POPOVER_MAX_WIDTH`], [`POPOVER_MAX_WIDTH_FRACTION`] |
//! | `buildGeo` + `rectAtProgress` (the gooey neck) | [`PopoverMorph::Neck`] |
//! | `popover-morph.tsx`'s `clipHidden`/`clipShown` corner unfold | [`PopoverMorph::Corner`] |
//! | `popover-morph.tsx`'s own `sideOffset` default `8` | [`POPOVER_MORPH_OFFSET`] |
//! | `MORPH_CLIP_TRANSITION` `0.32s EASE_OUT` | [`POPOVER_MORPH_OPEN`] |
//! | `GOO_CLOSE_SPRING` `visualDuration 0.21` | [`POPOVER_MORPH_CLOSE`] |
//! | `ALIGN_ORIGIN` × side transform origin | the host's own [`crate::overlay::transform_origin`] |
//!
//! # One morph, two source rects
//!
//! Upstream ships this popover twice. `popover.tsx` melts a *neck* between the
//! trigger and the panel: `buildGeo` puts both rects in one box and
//! `rectAtProgress` lerps `{x, y, w, h, r}` between them, driving the clip the
//! surface is revealed through. `popover-morph.tsx` does the same thing from a
//! fixed source instead: an `inset(92% …)` sliver in the panel corner nearest
//! the trigger, unclipping to the whole panel.
//!
//! Both are therefore *one* mechanism — a rounded rect travelling from a source
//! rect to the panel's own rect, with the surface and its content revealed
//! through it — differing only in where the source rect comes from.
//! [`PopoverMorph`] is that axis, and [`PopoverPanelWidget`] runs the single
//! lerp for both.
//!
//! # Mounting
//!
//! The panel mounts through [`crate::overlay::anchored`], the seam's non-modal
//! host; this component builds no host of its own. The host contributes the
//! placement ([`crate::overlay::place`]), the light dismiss, the Escape arm and
//! the wrapper's own spring-in/eased-out staging. This module contributes the
//! panel: its surface, hairline, shadow, padding, and the morph the whole thing
//! is revealed through.
//!
//! That split is upstream's own. `popover-morph.tsx` springs `{opacity, scale}`
//! on the portal *wrapper* while tweening the `clip-path` on the panel inside
//! it: the host's entrance is the wrapper spring, and the morph here is the
//! clip. The host's exit ramp is restated as [`POPOVER_MORPH_CLOSE`] so the two
//! settle together — a shorter host exit would unmount the pod with the morph
//! still folding.
//!
//! # Degradations against the web original
//!
//! - **No goo filter.** `popover.tsx`'s neck is an SVG `feGaussianBlur` +
//!   `feColorMatrix` contrast pair that melts the trigger pill and the panel
//!   into one blob. `PaintScene` has no filter primitive, so
//!   [`PopoverMorph::Neck`] keeps the neck's *geometry* — the same
//!   `rectAtProgress` lerp — and drops the melt: the surface travels and grows
//!   as one rounded rect with hard edges rather than a gooey one.
//! - **No trigger punch-out.** Upstream clips its own copy of the trigger pill
//!   back out of the goo layer so the real trigger's label stays legible under
//!   it. Nothing is portalled here and the panel is the only thing painted, so
//!   there is nothing to punch — but it also means the travelling surface
//!   passes *over* the trigger during the first frames of the neck morph
//!   instead of merging with it.
//! - **No hover trigger mode.** `popover.tsx`'s `trigger="hover"` and its
//!   `HOVER_CLOSE_DELAY` are not ported: a panel the pointer has to travel onto
//!   needs the pointer-leave event [`crate::overlay::anchored`] documents frust
//!   as not having. A click popover is the ported mode;
//!   [`crate::components::tooltip`] carries what hover opening this catalog
//!   does support.
//! - **All four sides are reachable**, where upstream's popover offers only
//!   `top`/`bottom` — the shared host's [`OverlaySide`] is the union of the
//!   popover's and the tooltip's, and the morph is defined for each.
//! - **Controlled only.** Upstream's uncontrolled `defaultOpen` half needs
//!   component-local state; every widget in this catalog is controlled, and the
//!   app owns the flag.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Rect, RoundedRect, SemanticsCtx, Shape,
    Size, View, Widget, any, build_child, rebuild_child, route_event_single, teardown_child,
    visit_children,
};

use crate::motion::{Presence, PresencePhase, Ramp};
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, SIDE_OFFSET, anchored,
};
use crate::style;
use crate::tokens::motion::EASE_OUT;

/// `panelRadius` — the open panel's corner radius, in logical px.
pub const POPOVER_RADIUS: f64 = 16.0;

/// `p-4` — the panel's own padding around its content, in logical px.
pub const POPOVER_PADDING: f64 = 16.0;

/// `20rem` — the panel's width cap, in logical px.
pub const POPOVER_MAX_WIDTH: f64 = 320.0;

/// `92vw` — the share of the host area the panel may not exceed, whichever of
/// this and [`POPOVER_MAX_WIDTH`] binds first.
pub const POPOVER_MAX_WIDTH_FRACTION: f64 = 0.92;

/// `popover-morph.tsx`'s own `sideOffset` default, in logical px — the corner
/// unfold sits closer to its trigger than the neck does.
pub const POPOVER_MORPH_OFFSET: f64 = 8.0;

/// How long the clip morph takes to open (`MORPH_CLIP_TRANSITION`'s
/// `duration: 0.32`).
pub const POPOVER_MORPH_OPEN: Duration = Duration::from_millis(320);

/// How long the clip morph takes to close (`GOO_CLOSE_SPRING`'s
/// `visualDuration: 0.21`) — leaving reads quicker than arriving, upstream's
/// own asymmetry.
pub const POPOVER_MORPH_CLOSE: Duration = Duration::from_millis(210);

/// The share of the panel a [`PopoverMorph::Corner`] fold starts from —
/// `clipHidden`'s `92%` inset leaves 8% of each axis showing.
pub const POPOVER_CORNER_FRACTION: f64 = 0.08;

/// Where the panel's surface morphs out of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PopoverMorph {
    /// `popover.tsx`: out of the **trigger's own rect** — `buildGeo`'s gooey
    /// neck, minus the goo (see the [module docs](self)). The default.
    #[default]
    Neck,
    /// `popover-morph.tsx`: out of the **panel corner nearest the trigger** —
    /// `clipHidden`'s `inset(92% …)` sliver unfolding to the whole panel.
    Corner,
}

// ---- Shared panel chrome ---------------------------------------------------

/// The resolved chrome of an overlay panel: the surface it fills, the hairline
/// it strokes, and the two inks its own text is painted in.
///
/// Shared by this catalog's overlay family — the popover, the
/// [tooltip](crate::components::tooltip), the [context
/// menu](crate::components::context_menu) and the four modal surfaces — every
/// one of which is the same `border border-border bg-… shadow-…` recipe over a
/// different fill. It lives here because the popover is the family's plainest
/// panel, not because the others are built out of a popover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PanelChrome {
    /// The panel's fill.
    pub(crate) surface: Color,
    /// The hairline around it (`border-border`).
    pub(crate) border: Color,
    /// Ink for the panel's own text (`text-foreground`).
    pub(crate) ink: Color,
    /// The dimmed ink role (`text-muted-foreground`).
    pub(crate) dim_ink: Color,
    /// The destructive ink role (`text-destructive`).
    pub(crate) danger_ink: Color,
}

/// The `y_offset` of the `glass` chrome tier's drop shadow, in logical px.
///
/// Restated here rather than read off the live `ShadowSpec`, for the reason
/// `crate::overlay::modal` restates the same pair: the theme builds that spec
/// inside a closure with no standalone constant to import. Both overlay hosts
/// size their fade layers from these two numbers, so a panel painting anything
/// else would spill outside the room they reserved.
pub(crate) const PANEL_SHADOW_Y_OFFSET: f64 = 24.0;

/// The blur standard deviation of that same shadow, in logical px.
pub(crate) const PANEL_SHADOW_BLUR: f64 = 30.0;

/// Its alpha (`.glass`'s `0 24px 60px -24px rgb(0 0 0 / 0.45)`).
pub(crate) const PANEL_SHADOW_ALPHA: f32 = 0.45;

/// Resolve a panel's chrome from `theme`, falling back to the vendored
/// **light** table unthemed — the per-value fallback rule the crate charter
/// sets.
///
/// `surface` is the `popover` role (`--popover`, itself an alias of `--card`),
/// which is what every overlay panel upstream fills with.
pub(crate) fn resolve_panel(theme: Option<&Theme>) -> PanelChrome {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            PanelChrome {
                surface: s.surface_container_high,
                border: s.outline_variant,
                ink: s.on_surface,
                dim_ink: s.on_surface_variant,
                danger_ink: s.error,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            PanelChrome {
                surface: p.popover,
                border: p.border,
                ink: p.foreground,
                dim_ink: p.muted_foreground,
                danger_ink: p.destructive,
            }
        }
    }
}

/// Paint an overlay panel's shadow, fill and hairline over `rect` (in
/// `origin`'s space) at `radius`.
pub(crate) fn paint_panel(
    scene: &mut dyn PaintScene,
    origin: Point,
    rect: Rect,
    radius: f64,
    chrome: PanelChrome,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let at = origin + rect.origin().to_vec2();
    scene.draw_shadow(
        Point::new(at.x, at.y + PANEL_SHADOW_Y_OFFSET),
        rect.size(),
        radius,
        PANEL_SHADOW_BLUR,
        style::with_alpha(Color::BLACK, PANEL_SHADOW_ALPHA),
    );
    scene.fill_rounded_rect(at, rect.size(), radius, chrome.surface);
    paint_panel_hairline(scene, at, rect.size(), radius, chrome.border);
}

/// Stroke a panel's 1px hairline just inside `size`, so it lands within the
/// surface rather than straddling its edge.
pub(crate) fn paint_panel_hairline(
    scene: &mut dyn PaintScene,
    at: Point,
    size: Size,
    radius: f64,
    color: Color,
) {
    let half = style::BORDER_WIDTH / 2.0;
    if size.width <= style::BORDER_WIDTH || size.height <= style::BORDER_WIDTH {
        return;
    }
    let hairline = RoundedRect::from_rect(
        Rect::from_origin_size(Point::ORIGIN, size).inset(-half),
        (radius - half).max(0.0),
    );
    scene.stroke_path(
        at,
        &Shape::to_path(&hairline, style::PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

/// Linear interpolation, `t` unclamped.
pub(crate) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// `rectAtProgress`: the rounded rect `progress` of the way from `from` to `to`.
///
/// Returns the rect and its radius, both lerped — upstream lerps `r` alongside
/// `{x, y, w, h}` so the travelling shape's corners resolve with it rather than
/// after it. `progress` is clamped: a spring overshoot past the panel's own
/// rect would tear the surface off its layout box.
pub(crate) fn morph_rect(
    from: Rect,
    from_radius: f64,
    to: Rect,
    to_radius: f64,
    progress: f64,
) -> (Rect, f64) {
    let t = progress.clamp(0.0, 1.0);
    let rect = Rect::new(
        lerp(from.x0, to.x0, t),
        lerp(from.y0, to.y0, t),
        lerp(from.x1, to.x1, t),
        lerp(from.y1, to.y1, t),
    );
    (rect, lerp(from_radius, to_radius, t))
}

// ---- The component ---------------------------------------------------------

/// The mutable panel configuration the outer builder writes and the inner panel
/// view reads.
///
/// The panel view is constructed inside [`popover`] before any setter has run,
/// so the setters cannot rebuild it; they write this shared cell instead — the
/// handle shape `plugins/shadcn/src/components/popover.rs` establishes for the
/// same reason.
type PanelHandle = Rc<RefCell<PanelConfig>>;

/// What the panel needs from its component's builders.
#[derive(Clone, Debug)]
struct PanelConfig {
    open: bool,
    morph: PopoverMorph,
    radius: f64,
    padding: f64,
    max_width: f64,
    anchor: OverlayAnchor,
    /// The side the panel was asked for, which decides which corner a
    /// [`PopoverMorph::Corner`] fold starts from.
    side: OverlaySide,
    align: OverlayAlign,
}

/// Equality over everything the panel *renders* from. [`OverlayAnchor`] is a
/// shared cell whose contents change on the trigger's every paint and which
/// carries no identity comparison, so it is deliberately excluded: the widget
/// reads it live at paint time, and the rebuild below re-seats the handle
/// unconditionally rather than diffing it.
impl PartialEq for PanelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.open == other.open
            && self.morph == other.morph
            && self.radius == other.radius
            && self.padding == other.padding
            && self.max_width == other.max_width
            && self.side == other.side
            && self.align == other.align
    }
}

impl Default for PanelConfig {
    fn default() -> Self {
        PanelConfig {
            open: true,
            morph: PopoverMorph::default(),
            radius: POPOVER_RADIUS,
            padding: POPOVER_PADDING,
            max_width: POPOVER_MAX_WIDTH,
            anchor: OverlayAnchor::new(),
            side: OverlaySide::Bottom,
            align: OverlayAlign::Center,
        }
    }
}

/// A declarative beUI popover. See [`popover`].
pub struct PopoverView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    config: PanelHandle,
    placement: OverlayPlacement,
}

/// Build a popover panel around `content`, to be mounted as the top child of a
/// full-area [`frust::Stack`] (or as a transparent navigator page) and handed
/// the app's own open flag through [`PopoverView::open`].
///
/// The panel is **kept mounted**: closing it plays the morph and the host's own
/// exit rather than making it vanish. Point it at a trigger with
/// [`PopoverView::anchor`], wrapping that trigger in
/// [`crate::overlay::anchor`].
pub fn popover<State: 'static, V: View<State>>(content: V) -> PopoverView<State> {
    let config: PanelHandle = Rc::new(RefCell::new(PanelConfig::default()));
    let placement = OverlayPlacement::default();
    let panel = PopoverPanelView {
        content: any(content),
        config: config.clone(),
    };
    PopoverView {
        inner: anchored(panel)
            .placement(placement)
            .exit(Ramp::eased(POPOVER_MORPH_CLOSE, EASE_OUT)),
        config,
        placement,
    }
}

impl<State: 'static> PopoverView<State> {
    /// Anchor the panel to the rect `anchor` carries — the cell
    /// [`crate::overlay::anchor`] writes on the trigger's every paint.
    ///
    /// The same cell feeds [`PopoverMorph::Neck`]'s source rect, which is why
    /// the panel keeps a clone of it rather than reading the host's.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.config.borrow_mut().anchor = anchor.clone();
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the panel opens on (default [`OverlaySide::Bottom`]).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.config.borrow_mut().side = side;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Set the cross-axis alignment (default [`OverlayAlign::Center`]).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.config.borrow_mut().align = align;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Set the gap between trigger and panel, in logical px (default
    /// [`SIDE_OFFSET`]).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Pick which morph the surface is revealed through (default
    /// [`PopoverMorph::Neck`]).
    ///
    /// [`PopoverMorph::Corner`] also adopts `popover-morph.tsx`'s own tighter
    /// [`POPOVER_MORPH_OFFSET`] gap, unless [`offset`](Self::offset) has already
    /// said otherwise.
    pub fn morph(mut self, morph: PopoverMorph) -> Self {
        self.config.borrow_mut().morph = morph;
        if morph == PopoverMorph::Corner && self.placement.offset == SIDE_OFFSET {
            self.placement.offset = POPOVER_MORPH_OFFSET;
            self.inner = self.inner.placement(self.placement);
        }
        self
    }

    /// Set the panel's corner radius, in logical px (default
    /// [`POPOVER_RADIUS`]).
    pub fn radius(self, radius: f64) -> Self {
        self.config.borrow_mut().radius = radius.max(0.0);
        self
    }

    /// Set the panel's padding around its content, in logical px (default
    /// [`POPOVER_PADDING`]).
    pub fn padding(self, padding: f64) -> Self {
        self.config.borrow_mut().padding = padding.max(0.0);
        self
    }

    /// Set the panel's width cap, in logical px (default
    /// [`POPOVER_MAX_WIDTH`]). [`POPOVER_MAX_WIDTH_FRACTION`] of the host area
    /// still applies on top of it.
    pub fn max_width(self, max_width: f64) -> Self {
        self.config.borrow_mut().max_width = max_width.max(0.0);
        self
    }

    /// Hand the panel the app's open flag. The default is `true`: a mounted
    /// popover is an open one.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: a press outside the panel, or Escape once
    /// the host holds focus, reports `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }

    /// Set the exit-finished callback — the host has settled closed and the
    /// whole overlay may be unmounted.
    pub fn on_exited<F: Fn(&mut State) + 'static>(mut self, on_exited: F) -> Self {
        self.inner = self.inner.on_exited(on_exited);
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

// ---- The panel -------------------------------------------------------------

/// The popover's panel: the surface, the hairline, the shadow, the padding and
/// the morph its content is revealed through.
struct PopoverPanelView<State: 'static> {
    content: AnyView<State>,
    config: PanelHandle,
}

/// The retained widget for a popover panel.
pub struct PopoverPanelWidget {
    content: ChildPod,
    config: PanelConfig,
    /// Drives the clip morph, opened and closed with the host.
    morph: Presence,
    /// The `reduce_motion` value `morph` was last built for; `None` until the
    /// first paint resolves a theme.
    reduced: Option<bool>,
    /// The morph's progress at the last paint.
    progress: Cell<f64>,
}

impl PopoverPanelWidget {
    /// The morph's progress at the last paint: `0.0` folded onto its source
    /// rect, `1.0` the whole panel.
    pub fn morph_progress(&self) -> f64 {
        self.progress.get()
    }

    /// The rect the morph starts from, in the panel's own space, and the radius
    /// it starts at.
    ///
    /// [`PopoverMorph::Neck`] reads the trigger's captured window-space rect and
    /// subtracts the panel's own absolute origin; [`PopoverMorph::Corner`] takes
    /// a [`POPOVER_CORNER_FRACTION`] sliver of the panel in the corner nearest
    /// the trigger, which is the side/align pair the placement was asked for.
    fn source_rect(&self, panel: Rect, origin: Point) -> (Rect, f64) {
        match self.config.morph {
            PopoverMorph::Neck => {
                let trigger = self.config.anchor.rect() - origin.to_vec2();
                // `buildGeo`'s `triggerRadius`: the trigger reads as a pill, up
                // to the panel's own radius.
                let radius = (trigger.height() / 2.0).min(self.config.radius).max(0.0);
                (trigger, radius)
            }
            PopoverMorph::Corner => {
                let w = panel.width() * POPOVER_CORNER_FRACTION;
                let h = panel.height() * POPOVER_CORNER_FRACTION;
                let leading = matches!(
                    self.config.align,
                    OverlayAlign::Start | OverlayAlign::Center
                );
                let (x0, y0) = match self.config.side {
                    // Below the trigger: the fold hangs off the panel's top edge.
                    OverlaySide::Bottom => {
                        (if leading { panel.x0 } else { panel.x1 - w }, panel.y0)
                    }
                    // Above it: off the bottom edge.
                    OverlaySide::Top => {
                        (if leading { panel.x0 } else { panel.x1 - w }, panel.y1 - h)
                    }
                    // Beside it: off the edge facing the trigger.
                    OverlaySide::Right => (panel.x0, if leading { panel.y0 } else { panel.y1 - h }),
                    OverlaySide::Left => {
                        (panel.x1 - w, if leading { panel.y0 } else { panel.y1 - h })
                    }
                };
                (
                    Rect::new(x0, y0, x0 + w, y0 + h),
                    // `clipHidden` keeps the panel's own radius throughout, and
                    // the upstream comment says why: a radius resolving
                    // separately spends the morph's last frames rounding
                    // corners instead of unfolding.
                    self.config.radius,
                )
            }
        }
    }

    /// Rebuild the morph driver when `reduce_motion` flips, preserving what the
    /// old one was doing — the replace-and-restore [`Presence::collapsed`]
    /// forces on both overlay hosts, for the same reason.
    fn sync_motion(&mut self, reduce: bool) {
        if self.reduced == Some(reduce) {
            return;
        }
        let was_exiting = self.morph.phase() == PresencePhase::Exiting;
        self.reduced = Some(reduce);
        let base = morph_presence();
        let mut next = if reduce { base.collapsed() } else { base };
        if was_exiting && !self.config.open {
            next.set_open(true);
        }
        next.set_open(self.config.open);
        self.morph = next;
    }
}

/// The morph's own driver: the clip tween in, the shorter one out.
fn morph_presence() -> Presence {
    Presence::new(
        Ramp::eased(POPOVER_MORPH_OPEN, EASE_OUT),
        Ramp::eased(POPOVER_MORPH_CLOSE, EASE_OUT),
    )
}

impl<State: 'static> View<State> for PopoverPanelView<State> {
    type Element = PopoverPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PopoverPanelWidget {
        let config = self.config.borrow().clone();
        let mut morph = morph_presence();
        morph.set_open(config.open);
        PopoverPanelWidget {
            content: build_child(&self.content, ctx),
            config,
            morph,
            reduced: None,
            progress: Cell::new(0.0),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PopoverPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        let config = self.config.borrow().clone();
        element.config.anchor = config.anchor.clone();
        if element.config != config {
            if element.config.open != config.open {
                element.morph.set_open(config.open);
            }
            let relayout = element.config.padding != config.padding
                || element.config.max_width != config.max_width;
            element.config = config;
            flags |= ChangeFlags::PAINT;
            if relayout {
                flags |= ChangeFlags::LAYOUT;
            }
        }
        flags
    }

    fn teardown(&self, element: &mut PopoverPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for PopoverPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let pad = self.config.padding;
        // `w-max max-w-[min(92vw, 20rem)]`: hug the content, capped by the
        // smaller of the fixed cap and the area share. The host lays the panel
        // out with a loose box the size of its whole area, so `bc.max()` is that
        // area.
        let area_width = bc.max().width;
        let cap = if area_width.is_finite() {
            self.config
                .max_width
                .min(area_width * POPOVER_MAX_WIDTH_FRACTION)
        } else {
            self.config.max_width
        }
        .max(0.0);
        let inner = Size::new(
            (cap - pad * 2.0).max(0.0),
            (bc.max().height - pad * 2.0).max(0.0),
        );
        let content = self
            .content
            .layout_child(ctx, &BoxConstraints::loose(inner));
        self.content.set_origin(Point::new(pad, pad));
        bc.constrain(Size::new(
            content.width + pad * 2.0,
            content.height + pad * 2.0,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let chrome = resolve_panel(Theme::from_paint_ctx(ctx));
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|theme| theme.motion.reduce_motion);
        self.sync_motion(reduce);

        let progress = self.morph.advance(ctx.frame_time());
        self.progress.set(progress);
        if self.morph.is_animating() {
            ctx.request_frame();
        }
        if !self.morph.is_visible() {
            // Fully folded away. The host may still be fading the pod out, but
            // there is no surface left to show.
            return;
        }

        let panel = Rect::from_origin_size(Point::ORIGIN, ctx.size());
        let (from, from_radius) = self.source_rect(panel, ctx.origin());
        let (rect, radius) = morph_rect(from, from_radius, panel, self.config.radius, progress);
        paint_panel(scene, ctx.origin(), rect, radius, chrome);
        // The content is revealed through the very same shape, which is what
        // makes the morph read as one surface unfolding rather than a panel
        // fading in behind a travelling rect.
        scene.push_clip_rounded(ctx.origin() + rect.origin().to_vec2(), rect.size(), radius);
        self.content.paint_child(ctx, scene);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.content.semantics_child(ctx);
    }

    visit_children!(content);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, KeyEvent, Modifiers, PointerButton, PointerEvent, PointerPhase,
    };
    use frust::authoring::{Key, NamedKey};
    use frust::{Brightness, FrameTime, SizedBox};
    use std::any::Any;

    /// A window big enough that no clamp binds on the placements under test.
    pub(crate) const WINDOW: Size = Size::new(600.0, 500.0);

    /// Records what a paint pass drew.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub(crate) rects: Vec<(Point, Size, Color)>,
        pub(crate) rrects: Vec<(Point, Size, f64, Color)>,
        pub(crate) clips: Vec<(Point, Size, f64)>,
        pub(crate) shadows: Vec<(Point, Size, f64)>,
        pub(crate) inks: Vec<Color>,
        pub(crate) strokes: usize,
        pub(crate) paths: usize,
        pub(crate) layers: Vec<f32>,
        /// The translation of every pushed transform, `(dx, dy)`.
        pub(crate) transforms: Vec<(f64, f64)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s, 0.0));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, r: f64) {
            self.clips.push((o, s, r));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, r: f64, _sd: f64, _c: Color) {
            self.shadows.push((o, s, r));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {
            self.paths += 1;
        }
        fn push_transform(&mut self, transform: frust::authoring::Affine) {
            let c = transform.as_coeffs();
            self.transforms.push((c[4], c[5]));
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// A frame time `ms` milliseconds in.
    pub(crate) fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// The light beUI theme.
    pub(crate) fn light() -> Theme {
        crate::theme().with_brightness(Brightness::Light)
    }

    /// The light beUI theme with motion turned off.
    pub(crate) fn reduced() -> Theme {
        let mut theme = light();
        theme.motion.reduce_motion = true;
        theme
    }

    /// A primary-button pointer event.
    pub(crate) fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// A secondary-button pointer event — the context gesture.
    pub(crate) fn secondary(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    /// An Escape key-down.
    pub(crate) fn escape() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[derive(Default)]
    struct App {
        opens: Vec<bool>,
        exited: u32,
    }

    /// The trigger rect the anchor cell is seeded with, in window space.
    const TRIGGER: Rect = Rect::new(220.0, 100.0, 320.0, 140.0);

    struct Harness {
        widget: AnchoredOverlayWidget,
        state: App,
        tcx: TextContext,
        anchor: OverlayAnchor,
        morph: PopoverMorph,
        open: bool,
    }

    impl Harness {
        fn new(morph: PopoverMorph) -> Self {
            let anchor = OverlayAnchor::new();
            anchor.set(TRIGGER);
            let mut counter = 0u64;
            let widget = View::<App>::build(
                &Self::view(&anchor, morph, true),
                &mut BuildCtx::new(&mut counter),
            );
            let mut h = Harness {
                widget,
                state: App::default(),
                tcx: TextContext::new(),
                anchor,
                morph,
                open: true,
            };
            h.layout();
            h
        }

        fn view(anchor: &OverlayAnchor, morph: PopoverMorph, open: bool) -> PopoverView<App> {
            popover(SizedBox(Some(200.0), Some(120.0)))
                .anchor(anchor)
                .morph(morph)
                .open(open)
                .on_open_change(|s: &mut App, next| s.opens.push(next))
                .on_exited(|s: &mut App| s.exited += 1)
        }

        fn layout(&mut self) -> Size {
            let mut ctx = LayoutCtx::with_text_context(&mut self.tcx as &mut dyn Any);
            self.widget
                .layout(&mut ctx, &BoxConstraints::new(Size::ZERO, WINDOW))
        }

        fn set_open(&mut self, open: bool) {
            let prev = Self::view(&self.anchor, self.morph, self.open);
            let next = Self::view(&self.anchor, self.morph, open);
            let mut counter = 0u64;
            View::<App>::rebuild(
                &next,
                &prev,
                &mut self.widget,
                &mut BuildCtx::new(&mut counter),
            );
            self.open = open;
            self.layout();
        }

        fn paint_with(&mut self, theme: &Theme, ms: f64) -> (Recorder, bool) {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ZERO, WINDOW, ft_ms(ms)).with_theme(theme);
            self.widget.paint(&mut ctx, &mut rec);
            (rec, ctx.needs_frame())
        }

        fn paint(&mut self, ms: f64) -> (Recorder, bool) {
            let theme = light();
            self.paint_with(&theme, ms)
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventResult {
            let mut ctx = EventCtx::new(&mut self.state as &mut dyn Any, Point::ZERO, WINDOW);
            self.widget.event(&mut ctx, event)
        }
    }

    // ---- Pure geometry ----------------------------------------------------

    #[test]
    fn the_morph_lerps_every_edge_and_the_radius_together() {
        let from = Rect::new(10.0, 10.0, 30.0, 20.0);
        let to = Rect::new(0.0, 40.0, 200.0, 160.0);
        let (mid, radius) = morph_rect(from, 5.0, to, 16.0, 0.5);
        assert_eq!(mid, Rect::new(5.0, 25.0, 115.0, 90.0));
        assert!((radius - 10.5).abs() < 1e-9);

        let (start, r0) = morph_rect(from, 5.0, to, 16.0, 0.0);
        assert_eq!(start, from);
        assert_eq!(r0, 5.0);
        let (end, r1) = morph_rect(from, 5.0, to, 16.0, 1.0);
        assert_eq!(end, to);
        assert_eq!(r1, 16.0);
    }

    #[test]
    fn the_morph_clamps_an_overshoot_back_onto_the_panel() {
        let from = Rect::new(0.0, 0.0, 10.0, 10.0);
        let to = Rect::new(0.0, 0.0, 100.0, 100.0);
        let (over, _) = morph_rect(from, 0.0, to, 8.0, 1.2);
        assert_eq!(over, to, "past 1.0 the morph rests on the panel");
    }

    #[test]
    fn the_panel_chrome_falls_back_to_the_vendored_light_table_unthemed() {
        let unthemed = resolve_panel(None);
        assert_eq!(unthemed.surface, crate::BEUI_LIGHT.popover);
        assert_eq!(unthemed.border, crate::BEUI_LIGHT.border);
        let theme = light();
        assert_eq!(
            resolve_panel(Some(&theme)).surface,
            theme.scheme().surface_container_high
        );
    }

    // ---- Layout -----------------------------------------------------------

    #[test]
    fn the_panel_hugs_its_content_inside_its_own_padding() {
        let mut h = Harness::new(PopoverMorph::Neck);
        h.paint(0.0);
        let panel = h.widget.content_rect();
        assert_eq!(panel.width(), 200.0 + POPOVER_PADDING * 2.0);
        assert_eq!(panel.height(), 120.0 + POPOVER_PADDING * 2.0);
    }

    #[test]
    fn the_panel_is_capped_by_the_smaller_of_the_fixed_cap_and_the_area_share() {
        // A 200px-wide area caps at 92% of it, well under `max-w-[20rem]`.
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(20.0, 20.0, 60.0, 40.0));
        let mut counter = 0u64;
        let mut widget = View::<App>::build(
            &popover::<App, _>(SizedBox(Some(600.0), Some(40.0))).anchor(&anchor),
            &mut BuildCtx::new(&mut counter),
        );
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let area = Size::new(200.0, 400.0);
        widget.layout(&mut ctx, &BoxConstraints::new(Size::ZERO, area));
        let expected = (area.width * POPOVER_MAX_WIDTH_FRACTION).min(POPOVER_MAX_WIDTH);
        assert_eq!(widget.content_rect().width(), expected);
    }

    // ---- The morph --------------------------------------------------------

    #[test]
    fn the_neck_morph_starts_on_the_triggers_own_rect() {
        let mut h = Harness::new(PopoverMorph::Neck);
        let (rec, frame) = h.paint(0.0);
        assert!(frame, "an unsettled morph owes another frame");
        let (origin, size, _, _) = *rec.rrects.first().expect("the panel's surface");
        assert!(
            (origin.x - TRIGGER.x0).abs() < 1.0 && (origin.y - TRIGGER.y0).abs() < 1.0,
            "the first frame's surface sits on the trigger, not the panel: {origin:?}"
        );
        assert!(
            (size.width - TRIGGER.width()).abs() < 1.0,
            "and is the trigger's width: {size:?}"
        );
    }

    #[test]
    fn the_neck_morph_lands_on_the_panels_own_rect() {
        let mut h = Harness::new(PopoverMorph::Neck);
        h.paint(0.0);
        // Long enough for both the morph tween and the host's own entrance
        // spring to settle.
        let (rec, frame) = h.paint(3_000.0);
        assert!(!frame, "a settled overlay owes nothing");
        let panel = h.widget.content_rect();
        let (origin, size, radius, _) = *rec.rrects.first().expect("the panel's surface");
        assert_eq!(origin, panel.origin());
        assert_eq!(size, panel.size());
        assert_eq!(radius, POPOVER_RADIUS);
    }

    #[test]
    fn the_corner_morph_starts_from_a_sliver_of_the_panel_not_the_trigger() {
        let mut h = Harness::new(PopoverMorph::Corner);
        let (rec, _) = h.paint(0.0);
        let panel = h.widget.content_rect();
        let (origin, size, _, _) = *rec.rrects.first().expect("the panel's surface");
        assert!(
            (size.width - panel.width() * POPOVER_CORNER_FRACTION).abs() < 1e-6,
            "the fold starts at 8% of the panel: {size:?}"
        );
        // Placed below the trigger and centre-aligned: the fold hangs off the
        // panel's own leading top corner, the edge facing the trigger.
        assert_eq!(origin, panel.origin());
    }

    #[test]
    fn the_corner_morph_takes_the_tighter_upstream_gap() {
        let anchor = OverlayAnchor::new();
        anchor.set(TRIGGER);
        let mut counter = 0u64;
        let mut widget = View::<App>::build(
            &popover::<App, _>(SizedBox(Some(100.0), Some(60.0)))
                .anchor(&anchor)
                .morph(PopoverMorph::Corner),
            &mut BuildCtx::new(&mut counter),
        );
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::new(Size::ZERO, WINDOW));
        assert_eq!(
            widget.content_rect().y0,
            TRIGGER.y1 + POPOVER_MORPH_OFFSET,
            "the corner variant adopts popover-morph.tsx's own 8px gap"
        );
    }

    #[test]
    fn the_content_is_revealed_through_the_very_same_shape() {
        let mut h = Harness::new(PopoverMorph::Neck);
        let (rec, _) = h.paint(0.0);
        let (surface_origin, surface_size, radius, _) = *rec.rrects.first().expect("surface");
        let (clip_origin, clip_size, clip_radius) = *rec.clips.first().expect("content clip");
        assert_eq!(clip_origin, surface_origin);
        assert_eq!(clip_size, surface_size);
        assert_eq!(clip_radius, radius);
    }

    #[test]
    fn reduce_motion_lands_the_morph_on_the_panel_immediately() {
        let mut h = Harness::new(PopoverMorph::Neck);
        let theme = reduced();
        let (rec, _) = h.paint_with(&theme, 0.0);
        let panel = h.widget.content_rect();
        let (origin, size, _, _) = *rec.rrects.first().expect("surface");
        assert_eq!(origin, panel.origin());
        assert_eq!(size, panel.size());
    }

    // ---- The state machine ------------------------------------------------

    #[test]
    fn a_press_outside_the_panel_reports_closed_and_a_closed_host_reports_nothing() {
        let mut h = Harness::new(PopoverMorph::Neck);
        h.paint(0.0);
        assert_eq!(
            h.dispatch(&pointer(PointerPhase::Down, 5.0, 5.0)),
            EventResult::Handled
        );
        assert_eq!(h.state.opens, vec![false]);
        // Once the app hands the flag back the host dismisses on nothing, which
        // is what makes the dismissal idempotent across the exit.
        h.set_open(false);
        h.paint(0.0);
        let before = h.state.opens.len();
        h.dispatch(&pointer(PointerPhase::Down, 5.0, 5.0));
        h.dispatch(&escape());
        assert_eq!(h.state.opens.len(), before);
    }

    #[test]
    fn escape_reports_closed_once_the_host_holds_focus() {
        let mut h = Harness::new(PopoverMorph::Neck);
        h.paint(0.0);
        let panel = h.widget.content_rect();
        h.dispatch(&pointer(
            PointerPhase::Down,
            panel.center().x,
            panel.center().y,
        ));
        h.dispatch(&pointer(
            PointerPhase::Up,
            panel.center().x,
            panel.center().y,
        ));
        h.state.opens.clear();
        h.dispatch(&escape());
        assert_eq!(h.state.opens, vec![false]);
    }

    #[test]
    fn the_close_morph_completes_before_the_host_lets_the_panel_go() {
        let mut h = Harness::new(PopoverMorph::Neck);
        h.paint(0.0);
        h.paint(400.0);
        h.set_open(false);
        // Mid-exit: the surface is still painted, on its way back to the trigger.
        let (mid, frame) = h.paint(500.0);
        assert!(frame, "the exit is still running");
        let (origin, _, _, _) = *mid.rrects.first().expect("the panel is still on screen");
        assert!(
            origin.y > h.widget.content_rect().y0 - 1.0,
            "the surface has started travelling back toward the trigger"
        );
        // Settled: nothing painted, and the exit reports itself on the next
        // pass. Both ramps are timed from the paint that started them (500ms).
        let done = 500.0 + POPOVER_MORPH_CLOSE.as_secs_f64() * 1000.0 + 20.0;
        let (after, still) = h.paint(done);
        assert!(!still);
        assert!(
            after.rrects.is_empty(),
            "a settled-closed panel paints nothing"
        );
        h.dispatch(&InputEvent::Housekeeping);
        assert_eq!(h.state.exited, 1);
    }

    #[test]
    fn reopening_mid_exit_puts_the_panel_back_without_a_second_exit_report() {
        let mut h = Harness::new(PopoverMorph::Neck);
        h.paint(0.0);
        h.paint(400.0);
        h.set_open(false);
        h.paint(450.0);
        h.set_open(true);
        let (rec, _) = h.paint(500.0);
        assert!(!rec.rrects.is_empty(), "the panel is back");
        h.dispatch(&InputEvent::Housekeeping);
        assert_eq!(h.state.exited, 0, "an interrupted exit never completed");
    }

    #[test]
    fn the_panel_casts_the_glass_chrome_shadow_the_hosts_reserve_room_for() {
        let mut h = Harness::new(PopoverMorph::Neck);
        let (rec, _) = h.paint(0.0);
        let (origin, _, _) = *rec.shadows.first().expect("the panel's shadow");
        let (surface, _, _, _) = rec.rrects[0];
        assert_eq!(origin.y - surface.y, PANEL_SHADOW_Y_OFFSET);
    }
}
