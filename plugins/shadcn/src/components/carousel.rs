//! Ports shadcn/ui's **Carousel** (the embla-backed slide pager) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/carousel.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! Upstream is a class list plus a context over `embla-carousel-react`: the
//! registry file contributes the layout and the two arrow buttons, the package
//! contributes the scrolling. There is no package to wrap here, so the paging is
//! re-implemented and the class lists translate like this:
//!
//! | class | here |
//! |---|---|
//! | content `overflow-hidden` + `flex` | a clipped viewport over one row/column of items |
//! | content `-ml-4` + item `pl-4` (`-mt-4`/`pt-4` vertical) | [`CAROUSEL_ITEM_GAP`] between items, none at the ends |
//! | item `min-w-0 shrink-0 grow-0 basis-full` | one viewport-wide item, or [`CarouselView::item_fraction`] of one |
//! | button `size-8 rounded-full`, `variant="outline"` | [`CAROUSEL_CONTROL_SIZE`] pills in the `outline` palette |
//! | button `-left-12` / `-right-12` (`-top-12`/`-bottom-12`) | [`CAROUSEL_CONTROL_GUTTER`] reserved at each end |
//! | button `disabled={!canScrollPrev}` | [`CarouselWidget::can_scroll_prev`], painted at [`style::DISABLED_OPACITY`] |
//! | `ArrowLeft`/`ArrowRight` | lucide's arrow, quarter-turned in a vertical carousel |
//!
//! # The controls sit *inside* the widget
//!
//! Upstream positions the buttons at `-left-12`/`-right-12`, i.e. outside the
//! carousel's own box, relying on the page to leave room. A frust widget can
//! paint outside its bounds but cannot be *hit* there (a pointer pass is routed
//! by bounds), so the same 48px band is reserved **inside** the widget instead:
//! the viewport is inset by [`CAROUSEL_CONTROL_GUTTER`] at both ends and each
//! control is centred in its gutter. The spatial relationship survives; the
//! overflow does not. [`CarouselView::controls`] turns the pair off, which also
//! reclaims the gutters.
//!
//! # Uncontrolled by default, controlled through `selected`
//!
//! embla owns the selected index upstream (`setApi` only *observes* it), and so
//! does this widget by default: a drag, a button or an arrow key moves its own
//! index and reports the new one through [`CarouselView::on_select`]. Calling
//! [`CarouselView::selected`] flips it to the catalog's usual controlled
//! contract — the widget then reports the *requested* index and moves nothing
//! until the next `rebuild` feeds the confirmed one back down.
//!
//! # Drag, and what ends it
//!
//! A press on the viewport captures the pointer ([`CursorIcon::Grab`] before,
//! [`CursorIcon::Grabbing`] during) and offsets the row with the pointer. The
//! release snaps to the nearest item, or one further along when the last move
//! crossed [`TOUCH_SLOP`] — a flick. There is no time-based velocity here: the
//! event pass carries no clock (a widget would have to sample the last *painted*
//! frame's time, which every move inside one frame shares), so the final move's
//! own distance is what stands in for one.
//!
//! A slide that consumes the press itself (a button inside a card) keeps the
//! drag from starting, since this widget routes to its items first and does not
//! implement the baseline scroll view's slop-takeover-and-cancel. Drag works on
//! ordinary slides; a slide full of controls is a pager you page with the
//! buttons.
//!
//! # Motion
//!
//! Settling to an item runs a [`CAROUSEL_SNAP_MS`] ease-out ramp — embla's own
//! `duration` is a scroll-body coefficient with no millisecond meaning outside
//! its physics, so the duration is this port's decision. Under
//! `Theme.motion.reduce_motion` the row jumps to the item instead, and asks for
//! no further frames.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CursorIcon, ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size,
    View, Widget, build_child, erase_callback_arg, rebuild_children, route_event, teardown_child,
    visit_children,
};
use frust::input::TOUCH_SLOP;
use frust::{AnimationController, Curve, Theme};

use crate::hit::{inside, presses};
use crate::style::{self, PATH_TOLERANCE};

/// Gap between items, in logical px (`-ml-4` on the row plus `pl-4` on each
/// item, which is upstream's way of gapping without a trailing edge).
pub const CAROUSEL_ITEM_GAP: f64 = 16.0;

/// Control diameter, in logical px (`size-8`).
pub const CAROUSEL_CONTROL_SIZE: f64 = 32.0;

/// The band reserved at each end of the main axis for a control, in logical px
/// (`-left-12`/`-right-12`). See the [module docs](self) for why it is inside
/// the widget rather than outside it.
pub const CAROUSEL_CONTROL_GUTTER: f64 = 48.0;

/// How long the row takes to settle onto an item, in ms — a port decision (see
/// the [module docs](self)).
pub const CAROUSEL_SNAP_MS: u64 = 250;

/// Offset differences below which the row counts as settled, in logical px.
const OFFSET_EPSILON: f64 = 1e-3;

/// Side of the lucide viewBox the arrow's coordinates are authored in.
const ICON_VIEWBOX: f64 = 24.0;
/// Lucide's uniform stroke width, in viewBox units.
const ICON_STROKE_VIEWBOX: f64 = 2.0;

/// The `axis` the source derives from its `orientation` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarouselOrientation {
    /// `axis: "x"` — items flow left to right.
    #[default]
    Horizontal,
    /// `axis: "y"` — items flow top to bottom.
    Vertical,
}

/// Which end of the carousel a control sits at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    /// `CarouselPrevious`.
    Prev,
    /// `CarouselNext`.
    Next,
}

/// A view-held, typed selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative shadcn carousel. See the [module docs](self).
pub struct CarouselView<State: 'static> {
    items: Vec<AnyView<State>>,
    orientation: CarouselOrientation,
    item_fraction: f64,
    controls: bool,
    selected: Option<usize>,
    on_select: Option<OnSelect<State>>,
}

/// Page through `items` one viewport-wide slide at a time, horizontally by
/// default — **uncontrolled** unless [`CarouselView::selected`] says otherwise
/// (see the [module docs](self)).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn carousel<State: 'static, V: View<State>>(
    items: impl IntoIterator<Item = V>,
) -> CarouselView<State> {
    let items: Vec<AnyView<State>> = items.into_iter().map(AnyView::new).collect();
    CarouselView {
        items,
        orientation: CarouselOrientation::default(),
        item_fraction: 1.0,
        controls: true,
        selected: None,
        on_select: None,
    }
}

impl<State: 'static> CarouselView<State> {
    /// Set the axis (`orientation`).
    pub fn orientation(mut self, orientation: CarouselOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// How much of the viewport one item takes (`basis-full` = `1.0`, the
    /// default; `basis-1/3` = `1.0 / 3.0`). Clamped to a sane sliver at the
    /// bottom so an item can never measure zero or negative.
    pub fn item_fraction(mut self, fraction: f64) -> Self {
        self.item_fraction = fraction.clamp(0.05, 1.0);
        self
    }

    /// Show the prev/next controls, and reserve the gutters they sit in
    /// (default `true`).
    pub fn controls(mut self, controls: bool) -> Self {
        self.controls = controls;
        self
    }

    /// Drive the carousel from the app: it shows exactly this item and reports
    /// every requested index through [`on_select`](Self::on_select) instead of
    /// paging itself (see the [module docs](self)).
    pub fn selected(mut self, index: usize) -> Self {
        self.selected = Some(index);
        self
    }

    /// Report the index every page lands on — the `setApi`/`select` pair,
    /// collapsed into the one callback an app actually wants.
    pub fn on_select<F: Fn(&mut State, usize) + 'static>(mut self, on_select: F) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }
}

/// The resolved control palette — the `outline` button variant, which is what
/// both controls are.
struct ControlColors {
    /// `bg-background`.
    fill: Color,
    /// `border`.
    border: Color,
    /// The arrow (`text-foreground`).
    ink: Color,
    /// `hover:bg-accent`.
    hover_fill: Color,
    /// `hover:text-accent-foreground`.
    hover_ink: Color,
}

/// Resolve the control palette, falling back to the `neutral` preset's light
/// values with no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> ControlColors {
    let scheme = theme.map_or_else(crate::tokens::color_scheme_light, |t| *t.scheme());
    ControlColors {
        fill: scheme.surface,
        border: scheme.outline,
        ink: scheme.on_surface,
        hover_fill: scheme.primary_container,
        hover_ink: scheme.on_primary_container,
    }
}

/// Paint lucide's `ArrowLeft` (`M19 12H5` plus the head `m12 19-7-7 7-7`)
/// centred on `center`, `extent` px on a side and rotated `angle` radians —
/// which is how the same glyph serves all four directions the source needs.
fn draw_arrow(scene: &mut dyn PaintScene, center: Point, extent: f64, angle: f64, color: Color) {
    let scale = extent / ICON_VIEWBOX;
    let (sin, cos) = angle.sin_cos();
    let at = |x: f64, y: f64| {
        let (x, y) = (x * scale, y * scale);
        Point::new(x * cos - y * sin, x * sin + y * cos)
    };
    // Both runs are viewBox-relative to its centre (12, 12).
    let mut shaft = BezPath::new();
    shaft.move_to(at(7.0, 0.0));
    shaft.line_to(at(-7.0, 0.0));
    let mut head = BezPath::new();
    head.move_to(at(0.0, 7.0));
    head.line_to(at(-7.0, 0.0));
    head.line_to(at(0.0, -7.0));
    let width = ICON_STROKE_VIEWBOX * scale;
    scene.stroke_path(center, &shaft, width, &Brush::Solid(color));
    scene.stroke_path(center, &head, width, &Brush::Solid(color));
}

impl<State: 'static> View<State> for CarouselView<State> {
    type Element = CarouselWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CarouselWidget {
        let index = self
            .selected
            .unwrap_or(0)
            .min(self.items.len().saturating_sub(1));
        CarouselWidget {
            pods: self.items.iter().map(|v| build_child(v, ctx)).collect(),
            orientation: self.orientation,
            item_fraction: self.item_fraction,
            controls: self.controls,
            controlled: self.selected.is_some(),
            index,
            offset: 0.0,
            anim_from: 0.0,
            anim: snap_controller(),
            main: 0.0,
            cross: 0.0,
            item_main: 0.0,
            drag: None,
            pressed: None,
            hovered: None,
            on_select: self.on_select.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CarouselWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.items,
            &self.items,
            &mut element.pods,
            ctx,
            |v| v,
            |_| None,
        );
        if element.orientation != self.orientation
            || element.item_fraction != self.item_fraction
            || element.controls != self.controls
        {
            element.orientation = self.orientation;
            element.item_fraction = self.item_fraction;
            element.controls = self.controls;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.controlled = self.selected.is_some();
        let last = element.pods.len().saturating_sub(1);
        if let Some(selected) = self.selected {
            // The app is the source of truth while controlled: adopt the
            // confirmed index and settle onto it.
            let selected = selected.min(last);
            if element.index != selected {
                element.index = selected;
                element.start_snap();
                flags |= ChangeFlags::PAINT;
            }
        } else if element.index > last {
            // A shrunk item list cannot leave the index past the end.
            element.index = last;
            element.start_snap();
            flags |= ChangeFlags::PAINT;
        }
        element.on_select = self.on_select.as_ref().map(erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut CarouselWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.items.iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// A fresh settle ramp at [`CAROUSEL_SNAP_MS`], eased out.
fn snap_controller() -> AnimationController {
    AnimationController::new(Duration::from_millis(CAROUSEL_SNAP_MS)).with_curve(Curve::EaseOut)
}

/// A live viewport drag.
struct Drag {
    /// The main-axis coordinate of the `Down`.
    start: f64,
    /// The row offset the drag started from.
    from: f64,
    /// The last move's own signed distance — this port's stand-in for a
    /// velocity (see the [module docs](self)).
    last_delta: f64,
    /// The main-axis coordinate of the previous move.
    last: f64,
}

/// The retained widget for a [`CarouselView`].
pub struct CarouselWidget {
    pods: Vec<ChildPod>,
    orientation: CarouselOrientation,
    item_fraction: f64,
    controls: bool,
    /// Whether the app drives the index (see the [module docs](self)).
    controlled: bool,
    /// The item the row is on (or settling onto).
    index: usize,
    /// The row's current main-axis scroll offset, in logical px.
    offset: f64,
    /// The offset the running settle started from.
    anim_from: f64,
    anim: AnimationController,
    /// Main/cross extents and the per-item main extent layout resolved — the
    /// event pass maps a pointer delta back onto items with them.
    main: f64,
    cross: f64,
    item_main: f64,
    drag: Option<Drag>,
    /// The control a `Down` armed, cleared on `Up`/`Cancel`.
    pressed: Option<Control>,
    /// The latched hovered control, self-corrected from `PaintCtx::is_hovered`
    /// every paint.
    hovered: Option<Control>,
    on_select: Option<ErasedArgCallback<usize>>,
}

impl CarouselWidget {
    /// Whether a previous item exists (`canScrollPrev`).
    pub fn can_scroll_prev(&self) -> bool {
        self.index > 0
    }

    /// Whether a next item exists (`canScrollNext`).
    pub fn can_scroll_next(&self) -> bool {
        self.index + 1 < self.pods.len()
    }

    /// The band reserved at each end for a control.
    fn gutter(&self) -> f64 {
        if self.controls {
            CAROUSEL_CONTROL_GUTTER
        } else {
            0.0
        }
    }

    /// The clipped viewport's main-axis extent.
    fn viewport(&self) -> f64 {
        (self.main - self.gutter() * 2.0).max(0.0)
    }

    /// The distance from one item's leading edge to the next's.
    fn stride(&self) -> f64 {
        self.item_main + CAROUSEL_ITEM_GAP
    }

    /// The row offset that puts item `index` at the viewport's leading edge.
    fn offset_for(&self, index: usize) -> f64 {
        index as f64 * self.stride()
    }

    /// A position's main-axis component under this carousel's orientation.
    fn main_of(&self, point: Point) -> f64 {
        match self.orientation {
            CarouselOrientation::Horizontal => point.x,
            CarouselOrientation::Vertical => point.y,
        }
    }

    /// The `(origin, size)` of a control, in widget-local coordinates.
    fn control_rect(&self, control: Control) -> (Point, Size) {
        let size = Size::new(CAROUSEL_CONTROL_SIZE, CAROUSEL_CONTROL_SIZE);
        let along = match control {
            Control::Prev => (CAROUSEL_CONTROL_GUTTER - CAROUSEL_CONTROL_SIZE) / 2.0,
            Control::Next => self.main - (CAROUSEL_CONTROL_GUTTER + CAROUSEL_CONTROL_SIZE) / 2.0,
        };
        let across = (self.cross - CAROUSEL_CONTROL_SIZE) / 2.0;
        let origin = match self.orientation {
            CarouselOrientation::Horizontal => Point::new(along, across),
            CarouselOrientation::Vertical => Point::new(across, along),
        };
        (origin, size)
    }

    /// The control under widget-local `pos`, if any (a disabled one still
    /// answers — the caller decides what a press on it means).
    fn control_at(&self, pos: Point) -> Option<Control> {
        if !self.controls {
            return None;
        }
        [Control::Prev, Control::Next].into_iter().find(|control| {
            let (origin, size) = self.control_rect(*control);
            Rect::from_origin_size(origin, size).contains(pos)
        })
    }

    /// Whether a control may be pressed at all.
    fn control_enabled(&self, control: Control) -> bool {
        match control {
            Control::Prev => self.can_scroll_prev(),
            Control::Next => self.can_scroll_next(),
        }
    }

    /// Whether widget-local `pos` is inside the scrollable viewport (i.e. not
    /// in a control's gutter).
    fn in_viewport(&self, pos: Point, size: Size) -> bool {
        if !inside(pos, size) {
            return false;
        }
        let main = self.main_of(pos);
        main >= self.gutter() && main <= self.main - self.gutter()
    }

    /// Start settling from wherever the row is toward the current index.
    fn start_snap(&mut self) {
        self.anim_from = self.offset;
        self.anim = snap_controller();
        self.anim.forward();
    }

    /// Page to `index`: report it, and move there too unless the app is
    /// driving (see the [module docs](self)).
    fn select(&mut self, ctx: &mut EventCtx, index: usize) -> bool {
        let last = self.pods.len().saturating_sub(1);
        let index = index.min(last);
        if let Some(on_select) = self.on_select.as_mut() {
            on_select(ctx, index);
        }
        if !self.controlled {
            self.index = index;
            self.start_snap();
        } else {
            // Controlled: the row still has to settle back onto the confirmed
            // item if a drag left it between two.
            self.start_snap();
        }
        ctx.request_redraw();
        true
    }

    /// The item a released drag lands on: the nearest one, or the next along
    /// when the last move crossed the gesture slop (see the [module docs](self)).
    fn snap_target(&self, last_delta: f64) -> usize {
        let stride = self.stride();
        if stride <= 0.0 {
            return self.index;
        }
        let raw = self.offset / stride;
        let last = self.pods.len().saturating_sub(1);
        let index = if last_delta.abs() > TOUCH_SLOP {
            // A drag *back* along the axis (negative delta) moves forward.
            if last_delta < 0.0 {
                raw.floor() + 1.0
            } else {
                raw.ceil() - 1.0
            }
        } else {
            raw.round()
        };
        index.clamp(0.0, last as f64) as usize
    }

    /// Place the item pods for the current offset. Called from `layout`, from
    /// the drag arm (so a drag moves the row without a relayout, the same route
    /// the baseline scroll view takes) and from `paint` while settling.
    fn sync_origins(&mut self) {
        let gutter = self.gutter();
        let stride = self.stride();
        for (index, pod) in self.pods.iter_mut().enumerate() {
            let along = gutter + index as f64 * stride - self.offset;
            pod.set_origin(match self.orientation {
                CarouselOrientation::Horizontal => Point::new(along, 0.0),
                CarouselOrientation::Vertical => Point::new(0.0, along),
            });
        }
    }

    /// Set the latched hovered control, reporting whether it changed.
    fn set_hovered(&mut self, hovered: Option<Control>) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// Paint one control: the `outline` pill, its hover swap, and the arrow.
    fn paint_control(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        control: Control,
        colors: &ControlColors,
        theme: Option<&Theme>,
    ) {
        let (local, size) = self.control_rect(control);
        let at = Point::new(origin.x + local.x, origin.y + local.y);
        let enabled = self.control_enabled(control);
        let active = enabled && (self.hovered == Some(control) || self.pressed == Some(control));
        let tint = |color: Color| style::disabled_tint(color, !enabled);
        let radius = CAROUSEL_CONTROL_SIZE / 2.0;

        style::draw_shadow(scene, at, size, radius, style::SHADOW_XS, theme);
        let (fill, ink) = if active {
            (colors.hover_fill, colors.hover_ink)
        } else {
            (colors.fill, colors.ink)
        };
        scene.fill_rounded_rect(at, size, radius, tint(fill));

        let inset = style::BORDER_WIDTH / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
            radius + inset,
        );
        scene.stroke_path(
            at,
            &Shape::to_path(&outline, PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(tint(colors.border)),
        );

        // One arrow glyph, turned to face the direction it pages.
        let quarter = std::f64::consts::FRAC_PI_2;
        let angle = match (self.orientation, control) {
            (CarouselOrientation::Horizontal, Control::Prev) => 0.0,
            (CarouselOrientation::Horizontal, Control::Next) => std::f64::consts::PI,
            (CarouselOrientation::Vertical, Control::Prev) => quarter,
            (CarouselOrientation::Vertical, Control::Next) => -quarter,
        };
        draw_arrow(
            scene,
            Point::new(at.x + radius, at.y + radius),
            style::ICON_SIZE,
            angle,
            tint(ink),
        );
    }
}

impl Widget for CarouselWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        // The row fills what it is offered on both axes; an unbounded axis has
        // no intrinsic extent to fall back on in a fraction-based layout.
        let size = Size::new(
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
        );
        let (main, cross) = match self.orientation {
            CarouselOrientation::Horizontal => (size.width, size.height),
            CarouselOrientation::Vertical => (size.height, size.width),
        };
        self.main = main;
        self.cross = cross;
        self.item_main = self.viewport() * self.item_fraction;

        let item_size = match self.orientation {
            CarouselOrientation::Horizontal => Size::new(self.item_main, cross),
            CarouselOrientation::Vertical => Size::new(cross, self.item_main),
        };
        for pod in self.pods.iter_mut() {
            pod.layout_child(ctx, &BoxConstraints::tight(item_size));
        }
        // A resize moves every item, and the row must stay on its own index.
        if !self.anim.is_animating() && self.drag.is_none() {
            self.offset = self.offset_for(self.index);
        }
        self.sync_origins();
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();
        // One scope for every theme read: the `&Theme` borrows the context, and
        // the frame request plus the child paints below need it mutably.
        let (colors, reduce_motion) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };

        // Settle onto the selected item. The row's own children move with the
        // offset, which this widget applies itself — no relayout is owed, so the
        // continuation is a bare paint frame.
        if self.drag.is_none() {
            let target = self.offset_for(self.index);
            let next = if reduce_motion {
                if self.anim.is_animating() {
                    self.anim.stop();
                }
                target
            } else if self.anim.is_animating() {
                if self.anim.advance(now) {
                    ctx.request_frame();
                }
                let ramp = self.anim.value_clamped();
                self.anim_from + (target - self.anim_from) * ramp
            } else {
                self.offset
            };
            if (next - self.offset).abs() > OFFSET_EPSILON {
                self.offset = next;
                self.sync_origins();
            }
        }

        // `overflow-hidden`: the items are clipped to the viewport between the
        // two control gutters.
        let gutter = self.gutter();
        let (clip_origin, clip_size) = match self.orientation {
            CarouselOrientation::Horizontal => (
                Point::new(origin.x + gutter, origin.y),
                Size::new(self.viewport(), size.height),
            ),
            CarouselOrientation::Vertical => (
                Point::new(origin.x, origin.y + gutter),
                Size::new(size.width, self.viewport()),
            ),
        };
        scene.push_clip(clip_origin, clip_size);
        for pod in &mut self.pods {
            pod.paint_child(ctx, scene);
        }
        scene.pop_clip();

        if self.controls {
            let theme = Theme::from_paint_ctx(ctx);
            self.paint_control(scene, origin, Control::Prev, &colors, theme);
            self.paint_control(scene, origin, Control::Next, &colors, theme);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return route_event(&mut self.pods, ctx, event);
        }
        let size = ctx.size();

        // A live drag or an armed control owns every pointer pass until it ends.
        if let InputEvent::Pointer(p) = event {
            if let Some(drag) = &mut self.drag {
                return match p.phase {
                    PointerPhase::Move => {
                        // The captured arm re-asks every move, which is what
                        // keeps `Grabbing` alive outside the widget's bounds.
                        ctx.set_cursor(CursorIcon::Grabbing);
                        let main = match self.orientation {
                            CarouselOrientation::Horizontal => p.position.x,
                            CarouselOrientation::Vertical => p.position.y,
                        };
                        drag.last_delta = main - drag.last;
                        drag.last = main;
                        self.offset = drag.from - (main - drag.start);
                        self.sync_origins();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        let last_delta = drag.last_delta;
                        self.drag = None;
                        let target = self.snap_target(last_delta);
                        self.select(ctx, target);
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        // Internal flags only, then settle back onto the item the
                        // app last confirmed — a cancel selects nothing.
                        self.drag = None;
                        self.start_snap();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Down => EventResult::Handled,
                };
            }
            if let Some(control) = self.pressed {
                return match p.phase {
                    PointerPhase::Move => {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        self.pressed = None;
                        if self.control_at(p.position) == Some(control) {
                            let index = match control {
                                Control::Prev => self.index.saturating_sub(1),
                                Control::Next => self.index + 1,
                            };
                            self.select(ctx, index);
                        }
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        self.pressed = None;
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Down => EventResult::Handled,
                };
            }
        }

        // Controls first: they sit in the gutters, outside every item's bounds,
        // and a press on one is never a drag.
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
            && presses(p)
            && let Some(control) = self.control_at(p.position)
        {
            if !self.control_enabled(control) {
                // `disabled:pointer-events-none`, and a press that lands here is
                // still not the row's.
                return EventResult::Ignored;
            }
            self.pressed = Some(control);
            ctx.capture_pointer();
            ctx.request_focus();
            ctx.request_redraw();
            return EventResult::Handled;
        }

        // Items next: a slide owns its own events, and this widget's hover claim
        // comes after the routing.
        let routed = route_event(&mut self.pods, ctx, event);
        if routed == EventResult::Handled {
            return routed;
        }

        match event {
            InputEvent::Key(key) => {
                let step = match (self.orientation, &key.key) {
                    (CarouselOrientation::Horizontal, Key::Named(NamedKey::ArrowRight))
                    | (CarouselOrientation::Vertical, Key::Named(NamedKey::ArrowDown)) => 1i64,
                    (CarouselOrientation::Horizontal, Key::Named(NamedKey::ArrowLeft))
                    | (CarouselOrientation::Vertical, Key::Named(NamedKey::ArrowUp)) => -1,
                    _ => return EventResult::Ignored,
                };
                // The source's own handler is horizontal-only; the vertical pair
                // is this port's completion of it.
                let next = (self.index as i64 + step).max(0) as usize;
                if next != self.index || !self.controlled {
                    self.select(ctx, next);
                }
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !self.in_viewport(p.position, size) || self.pods.is_empty() {
                        return EventResult::Ignored;
                    }
                    let main = self.main_of(p.position);
                    self.drag = Some(Drag {
                        start: main,
                        from: self.offset,
                        last_delta: 0.0,
                        last: main,
                    });
                    self.anim.stop();
                    ctx.capture_pointer();
                    // Focus is what routes the arrow keys here.
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    // The hover/cursor pass, claimed *after* routing so a
                    // slide's own claim wins.
                    let over = self.control_at(p.position);
                    if inside(p.position, size) {
                        ctx.claim_hover();
                        match over {
                            Some(control) if self.control_enabled(control) => {
                                ctx.set_cursor(style::ACTIVE_CURSOR);
                            }
                            None if self.in_viewport(p.position, size) => {
                                ctx.set_cursor(CursorIcon::Grab);
                            }
                            _ => {}
                        }
                    }
                    if self.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up | PointerPhase::Cancel => EventResult::Ignored,
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="region" aria-roledescription="carousel"`, with each slide's own
        // subtree inside it and the two controls' `sr-only` labels alongside.
        ctx.push_container(
            Role::Region,
            |_| {},
            |ctx| {
                for pod in &self.pods {
                    pod.semantics_child(ctx);
                }
                if self.controls {
                    for (control, label) in [
                        (Control::Prev, "Previous slide"),
                        (Control::Next, "Next slide"),
                    ] {
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(label);
                            if self.control_enabled(control) {
                                node.add_action(Action::Click);
                            } else {
                                node.set_disabled();
                            }
                        });
                    }
                }
            },
        );
    }

    visit_children!(pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{
        EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate, any,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(500.0, 200.0);
    /// The viewport once both gutters are reserved.
    const VIEWPORT: f64 = WINDOW.width - CAROUSEL_CONTROL_GUTTER * 2.0;

    #[derive(Default)]
    struct Recorder {
        clips: Vec<(Point, Size)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _sd: f64, _c: Color) {}
    }

    #[derive(Default)]
    struct Pages {
        index: usize,
        selections: Vec<usize>,
    }

    /// A leaf that fills whatever it is given, generic over the app state (the
    /// shared `test_support::leaf` is `View<()>` only).
    struct Slide;

    /// The retained half of [`Slide`].
    struct SlideWidget;

    impl<S: 'static> View<S> for Slide {
        type Element = SlideWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SlideWidget {
            SlideWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SlideWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for SlideWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    fn slides(count: usize) -> Vec<AnyView<Pages>> {
        (0..count).map(|_| any(Slide)).collect()
    }

    fn view(count: usize) -> CarouselView<Pages> {
        carousel(slides(count)).on_select(|s: &mut Pages, index: usize| {
            s.index = index;
            s.selections.push(index);
        })
    }

    fn build(v: &CarouselView<Pages>) -> CarouselWidget {
        let mut counter = 0u64;
        View::<Pages>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn laid_out(v: &CarouselView<Pages>) -> (CarouselWidget, Size) {
        let mut w = build(v);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(WINDOW));
        (w, size)
    }

    fn paint_at(w: &mut CarouselWidget, size: Size, theme: &Theme, ms: f64) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ZERO, size, FrameTime::from_nanos((ms * 1e6) as u64))
                .with_theme(theme);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        w: &mut CarouselWidget,
        state: &mut Pages,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    /// The main-axis centre of the viewport, where a drag starts.
    fn viewport_center() -> (f64, f64) {
        (WINDOW.width / 2.0, WINDOW.height / 2.0)
    }

    // ---- Layout ------------------------------------------------------------

    #[test]
    fn items_are_viewport_wide_and_gapped_inside_the_control_gutters() {
        let (w, size) = laid_out(&view(3));
        assert_eq!(size, WINDOW);
        assert_eq!(w.item_main, VIEWPORT, "`basis-full`");
        assert_eq!(w.pods[0].origin(), Point::new(CAROUSEL_CONTROL_GUTTER, 0.0));
        assert_eq!(
            w.pods[1].origin().x - w.pods[0].origin().x,
            VIEWPORT + CAROUSEL_ITEM_GAP
        );
    }

    #[test]
    fn a_fraction_basis_shows_several_items_at_once() {
        let (w, _) = laid_out(&view(4).item_fraction(1.0 / 3.0));
        assert!((w.item_main - VIEWPORT / 3.0).abs() < 1e-9);
        // Three items still fit inside one viewport, gaps included.
        assert!(w.item_main * 3.0 <= VIEWPORT);
    }

    #[test]
    fn dropping_the_controls_reclaims_their_gutters() {
        let (w, _) = laid_out(&view(2).controls(false));
        assert_eq!(w.item_main, WINDOW.width);
        assert_eq!(w.pods[0].origin(), Point::ZERO);
        assert_eq!(w.control_at(Point::new(4.0, 100.0)), None);
    }

    #[test]
    fn a_vertical_carousel_stacks_its_items() {
        let (w, _) = laid_out(&view(3).orientation(CarouselOrientation::Vertical));
        let viewport = WINDOW.height - CAROUSEL_CONTROL_GUTTER * 2.0;
        assert_eq!(w.item_main, viewport);
        assert_eq!(w.pods[0].origin(), Point::new(0.0, CAROUSEL_CONTROL_GUTTER));
        assert_eq!(
            w.pods[1].origin().y - w.pods[0].origin().y,
            viewport + CAROUSEL_ITEM_GAP
        );
    }

    #[test]
    fn an_unbounded_axis_lays_out_at_zero_rather_than_infinity() {
        let mut w = build(&view(2));
        let mut ctx = LayoutCtx::new();
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 200.0)),
        );
        assert_eq!(size.width, 0.0);
        assert_eq!(w.item_main, 0.0);
    }

    // ---- Paging ------------------------------------------------------------

    #[test]
    fn the_controls_page_one_item_at_a_time_and_disable_at_the_ends() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        assert!(!w.can_scroll_prev());
        assert!(w.can_scroll_next());

        // The prev control is inert at the front: no capture, no report.
        let (prev_origin, control) = w.control_rect(Control::Prev);
        let prev_at = Point::new(
            prev_origin.x + control.width / 2.0,
            prev_origin.y + control.height / 2.0,
        );
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(PointerPhase::Down, prev_at.x, prev_at.y)
            ),
            EventResult::Ignored
        );
        assert!(state.selections.is_empty());

        let (next_origin, _) = w.control_rect(Control::Next);
        let next_at = Point::new(
            next_origin.x + control.width / 2.0,
            next_origin.y + control.height / 2.0,
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, next_at.x, next_at.y),
        );
        assert_eq!(w.pressed, Some(Control::Next));
        assert!(state.selections.is_empty(), "never on down");
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, next_at.x, next_at.y),
        );
        assert_eq!(state.selections, vec![1]);
        assert_eq!(w.index, 1, "uncontrolled: the widget owns the index");
        assert!(w.can_scroll_prev());

        // A release away from the control fires nothing.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, next_at.x, next_at.y),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, viewport_center().0, 10.0),
        );
        assert_eq!(state.selections, vec![1]);
        assert_eq!(w.pressed, None);

        // ...and so does a cancelled press.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, next_at.x, next_at.y),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, next_at.x, next_at.y),
        );
        assert_eq!(state.selections, vec![1]);

        // At the far end, `next` goes inert too.
        w.index = 2;
        assert!(!w.can_scroll_next());
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(PointerPhase::Down, next_at.x, next_at.y)
            ),
            EventResult::Ignored
        );
    }

    #[test]
    fn arrow_keys_page_along_the_axis() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(state.selections, vec![1]);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(state.selections, vec![1, 0]);
        // The index never runs off either end.
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(w.index, 0);
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &key(Key::Named(NamedKey::ArrowDown))
            ),
            EventResult::Ignored,
            "the cross-axis arrows belong to somebody else"
        );

        let (mut vertical, size) = laid_out(&view(3).orientation(CarouselOrientation::Vertical));
        dispatch(
            &mut vertical,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(vertical.index, 1);
    }

    // ---- Drag --------------------------------------------------------------

    #[test]
    fn a_drag_moves_the_row_and_the_release_snaps_to_the_nearest_item() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        let (cx, cy) = viewport_center();
        let stride = w.stride();

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, cx, cy),
        );
        assert!(w.drag.is_some());
        // Drag most of the way to the next item, in small steps so the last
        // move is not itself a flick.
        let travel = stride * 0.6;
        for step in 1..=10 {
            let x = cx - travel * step as f64 / 10.0;
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(PointerPhase::Move, x, cy),
            );
        }
        assert!(
            (w.offset - travel).abs() < 1e-9,
            "the row followed the pointer"
        );
        assert_eq!(w.pods[0].origin().x, CAROUSEL_CONTROL_GUTTER - travel);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, cx - travel, cy),
        );
        assert_eq!(state.selections, vec![1], "past halfway, so it lands on 1");
        assert!(w.drag.is_none());
    }

    #[test]
    fn a_short_drag_falls_back_to_where_it_started() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        let (cx, cy) = viewport_center();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, cx, cy),
        );
        for step in 1..=4 {
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(PointerPhase::Move, cx - step as f64, cy),
            );
        }
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, cx - 4.0, cy),
        );
        assert_eq!(state.selections, vec![0], "nowhere near halfway");
    }

    #[test]
    fn a_flick_pages_even_when_the_row_barely_moved() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        let (cx, cy) = viewport_center();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, cx, cy),
        );
        // One move well past the gesture slop, then release.
        let flick = TOUCH_SLOP * 3.0;
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, cx - flick, cy),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, cx - flick, cy),
        );
        assert_eq!(
            state.selections,
            vec![1],
            "a flick carries to the next item"
        );

        // ...and back the other way.
        let mut back = w;
        back.index = 1;
        back.offset = back.offset_for(1);
        dispatch(
            &mut back,
            &mut state,
            size,
            &pointer(PointerPhase::Down, cx, cy),
        );
        dispatch(
            &mut back,
            &mut state,
            size,
            &pointer(PointerPhase::Move, cx + flick, cy),
        );
        dispatch(
            &mut back,
            &mut state,
            size,
            &pointer(PointerPhase::Up, cx + flick, cy),
        );
        assert_eq!(state.selections.last(), Some(&0));
    }

    #[test]
    fn a_cancelled_drag_selects_nothing_and_settles_back() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        let (cx, cy) = viewport_center();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, cx, cy),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, cx - 100.0, cy),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, cx - 100.0, cy),
        );
        assert!(state.selections.is_empty());
        assert!(w.drag.is_none());
        assert_eq!(w.index, 0);
    }

    #[test]
    fn a_press_in_a_control_gutter_is_never_a_drag() {
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        // Inside the gutter but off the control itself: neither a press nor a
        // drag.
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(PointerPhase::Down, 2.0, 4.0)
            ),
            EventResult::Ignored
        );
        assert!(w.drag.is_none());
    }

    // ---- Motion ------------------------------------------------------------

    #[test]
    fn the_row_eases_onto_the_selected_item_and_reduce_motion_jumps() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(&view(3));
        let mut state = Pages::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        let target = w.offset_for(1);

        paint_at(&mut w, size, &theme, 0.0); // seeds the clock
        paint_at(&mut w, size, &theme, CAROUSEL_SNAP_MS as f64 / 2.0);
        assert!(
            w.offset > 0.0 && w.offset < target,
            "mid-settle: {}",
            w.offset
        );
        paint_at(&mut w, size, &theme, CAROUSEL_SNAP_MS as f64 * 2.0);
        assert!((w.offset - target).abs() < OFFSET_EPSILON);

        let mut reduced = crate::theme();
        reduced.motion.reduce_motion = true;
        let (mut w, size) = laid_out(&view(3));
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        paint_at(&mut w, size, &reduced, 0.0);
        assert!(
            (w.offset - w.offset_for(1)).abs() < OFFSET_EPSILON,
            "reduce_motion jumps on the first frame"
        );
    }

    #[test]
    fn the_viewport_is_clipped_between_the_gutters() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(&view(3));
        let rec = paint_at(&mut w, size, &theme, 0.0);
        assert_eq!(rec.clips.len(), 1);
        assert_eq!(rec.clips[0].0, Point::new(CAROUSEL_CONTROL_GUTTER, 0.0));
        assert_eq!(rec.clips[0].1, Size::new(VIEWPORT, WINDOW.height));
    }

    #[test]
    fn both_controls_paint_as_outline_pills_and_dim_when_they_cannot_page() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let (mut w, size) = laid_out(&view(3));
        let rec = paint_at(&mut w, size, &theme, 0.0);

        assert_eq!(rec.rrects.len(), 2, "one pill per control");
        let (_, pill, radius, fill) = rec.rrects[0];
        assert_eq!(
            pill,
            Size::new(CAROUSEL_CONTROL_SIZE, CAROUSEL_CONTROL_SIZE)
        );
        assert_eq!(radius, CAROUSEL_CONTROL_SIZE / 2.0);
        // Prev cannot page at index 0, so its fill is halved.
        assert_eq!(fill.components[3], style::DISABLED_OPACITY);
        assert_eq!(rec.rrects[1].3, scheme.surface, "next is enabled");

        // Per control: the border plus the arrow's two runs.
        assert_eq!(rec.strokes.len(), 2 * 3);
    }

    #[test]
    fn rebuild_adopts_a_controlled_index_and_clamps_a_shrunk_list() {
        let prev = view(3).selected(0);
        let mut w = build(&prev);
        let mut counter = 0u64;
        let next = view(3).selected(2);
        let flags = View::<Pages>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.index, 2);
        assert!(flags.needs_paint());

        // A shrunk list pulls the index back inside it.
        let shrunk = view(1);
        View::<Pages>::rebuild(&shrunk, &next, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.index, 0);
        assert_eq!(w.pods.len(), 1);
    }

    #[test]
    fn a_controlled_carousel_reports_without_paging_itself() {
        let controlled = view(3).selected(0);
        let (mut w, size) = laid_out(&controlled);
        let mut state = Pages::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(state.selections, vec![1], "the request is reported");
        assert_eq!(w.index, 0, "the app owns the index");
    }

    // ---- Root-driven: cursor, semantics --------------------------------------

    struct Harness {
        root: RenderRoot<Pages, CarouselView<Pages>>,
        state: Pages,
    }

    impl Harness {
        fn new(count: usize) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: Pages::default(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Pages| view(count);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout(WINDOW);
            h
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn frame(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    /// The hover latch's three parts at once: the claim from the uncaptured
    /// `Move`, the latched flag that paints the swap, and the paint-time
    /// self-correction once the pointer leaves.
    #[test]
    fn hovering_an_enabled_control_swaps_its_fill_until_the_pointer_leaves() {
        let scheme = *crate::theme().scheme();
        let mut h = Harness::new(3);
        assert_eq!(h.frame().rrects[1].3, scheme.surface, "at rest");

        h.dispatch(&pointer(
            PointerPhase::Move,
            WINDOW.width - CAROUSEL_CONTROL_GUTTER / 2.0,
            WINDOW.height / 2.0,
        ));
        assert_eq!(
            h.frame().rrects[1].3,
            scheme.primary_container,
            "`hover:bg-accent`"
        );

        let (cx, cy) = viewport_center();
        h.dispatch(&pointer(PointerPhase::Move, cx, cy));
        assert_eq!(h.frame().rrects[1].3, scheme.surface, "and back off it");
    }

    #[test]
    fn the_cursor_is_grab_over_the_viewport_and_grabbing_while_dragging() {
        let mut h = Harness::new(3);
        let (cx, cy) = viewport_center();
        h.dispatch(&pointer(PointerPhase::Move, cx, cy));
        assert_eq!(h.root.cursor(), CursorIcon::Grab);

        // A captured drag keeps `Grabbing` well outside the widget.
        h.dispatch(&pointer(PointerPhase::Down, cx, cy));
        h.dispatch(&pointer(PointerPhase::Move, WINDOW.width + 400.0, 400.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grabbing);
        h.dispatch(&pointer(PointerPhase::Up, WINDOW.width + 400.0, 400.0));

        // The enabled control asks for the pointer; the disabled one asks for
        // nothing (`disabled:pointer-events-none`).
        let mut fresh = Harness::new(3);
        fresh.dispatch(&pointer(
            PointerPhase::Move,
            WINDOW.width - CAROUSEL_CONTROL_GUTTER / 2.0,
            WINDOW.height / 2.0,
        ));
        assert_eq!(fresh.root.cursor(), style::ACTIVE_CURSOR);
        fresh.dispatch(&pointer(
            PointerPhase::Move,
            CAROUSEL_CONTROL_GUTTER / 2.0,
            WINDOW.height / 2.0,
        ));
        assert_eq!(fresh.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn semantics_is_a_region_of_slides_with_two_labelled_controls() {
        let h = Harness::new(3);
        let update = h.semantics();
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Region),
            "`role=\"region\"`"
        );
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].1.label(), Some("Previous slide"));
        assert!(buttons[0].1.is_disabled(), "nothing to page back to");
        assert_eq!(buttons[1].1.label(), Some("Next slide"));
        assert!(buttons[1].1.supports_action(Action::Click));
    }

    #[test]
    fn visit_children_publishes_every_slide() {
        let w = build(&view(4));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 4);
    }
}
