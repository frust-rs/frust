//! Ports shadcn/ui's **Resizable** (the drag-resizable panel group) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/resizable.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! Upstream is a class list over `react-resizable-panels`: the registry file
//! styles the group, the panel and the separator, while the package contributes
//! the sizing. There is no package to wrap here, so the sizing is
//! re-implemented and the class lists translate like this:
//!
//! | class | here |
//! |---|---|
//! | group `flex h-full w-full`, `aria-[orientation=vertical]:flex-col` | [`ResizableDirection`] |
//! | handle `w-px bg-border` | [`RESIZABLE_HANDLE_THICKNESS`] filled with `outline_variant` |
//! | handle `after:w-1` (and `after:h-1` when horizontal) | [`RESIZABLE_HANDLE_HIT`], the invisible drag band |
//! | handle `focus-visible:ring-1 ring-ring ring-offset-1` | a 1px ring at 1px offset — **not** the catalog's 3px ring |
//! | grip `h-4 w-3 rounded-xs border bg-border` | [`GRIP_SIZE`]/[`GRIP_RADIUS`] over `outline_variant` |
//! | grip `GripVerticalIcon size-2.5` | six lucide dots, quarter-turned in a vertical group |
//!
//! # Sizes are fractions, and the group owns the arithmetic
//!
//! Every size here is a fraction of the group's main axis (`0.0..=1.0`), which
//! is `react-resizable-panels`' own percentage model divided by 100. Panels
//! without a [`ResizablePanel::default_size`] split what the sized ones leave,
//! and the whole list is normalized to sum to 1 — so a caller cannot author a
//! group that does not fill itself.
//!
//! # Uncontrolled by default, controlled through `sizes`
//!
//! The package is uncontrolled (`defaultSize` seeds it and the panels own their
//! sizes afterwards), and so is this port: a drag or a key rewrites the widget's
//! own fractions and reports them through
//! [`ResizablePanelGroupView::on_layout`]. Calling
//! [`ResizablePanelGroupView::sizes`] flips it to the catalog's usual controlled
//! contract — the widget then reports the *requested* layout and changes nothing
//! until the next `rebuild` feeds the confirmed fractions back down.
//!
//! # Handles are seams, not children
//!
//! Upstream interleaves `<ResizableHandle />` elements between panels, which lets
//! a caller author a handle count that cannot match the panel count. Here a group
//! of N panels always has N-1 seams and one [`ResizableHandle`] spec configures
//! all of them ([`ResizablePanelGroupView::handle`]) — the same design decision
//! `button_group` makes about its members' shared borders, for the same reason.
//!
//! # Cursor and keyboard
//!
//! The uncaptured `Move` arm asks for [`CursorIcon::ColResize`] (horizontal) or
//! [`CursorIcon::RowResize`] (vertical) while the pointer is in a seam's hit
//! band, and the **captured** arm re-asks on every move, which is what keeps the
//! shape while a drag wanders outside the group's own bounds. A press focuses the
//! seam it grabbed; the arrow keys along the group's axis then nudge it by
//! [`RESIZABLE_KEY_STEP`].

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, CursorIcon,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View,
    Widget, any, build_child, erase_callback_arg, rebuild_child, route_event, teardown_child,
    visit_children,
};

use crate::hit::presses;
use crate::style::{self, PATH_TOLERANCE};

/// The painted handle's thickness, in logical px (`w-px`).
pub const RESIZABLE_HANDLE_THICKNESS: f64 = 1.0;

/// The handle's invisible drag band, in logical px (`after:w-1` / `after:h-1`)
/// — the hit target a 1px line could never be.
pub const RESIZABLE_HANDLE_HIT: f64 = 4.0;

/// How far one arrow-key press moves a focused handle, as a fraction of the
/// group's main axis.
///
/// A port decision: the source file has no keyboard step of its own (it is
/// `react-resizable-panels`' `keyboardResizeBy`, which the shadcn class list
/// never names), so this is the catalog's stated choice rather than a
/// translation.
pub const RESIZABLE_KEY_STEP: f64 = 0.05;

/// The visible grip's `(width, height)` in logical px, in a horizontal group
/// (`w-3 h-4`); a vertical group swaps them.
const GRIP_SIZE: Size = Size::new(12.0, 16.0);

/// The grip's corner radius, in logical px (`rounded-xs`).
///
/// Tailwind's own `rounded-xs` (0.125rem): shadcn overrides `sm` upward through
/// `4xl` but adds no `xs` step, so this one is not a
/// [`ShadcnRadius`](crate::ShadcnRadius) rung.
const GRIP_RADIUS: f64 = 2.0;

/// The grip glyph's edge, in logical px (`size-2.5` on the `GripVerticalIcon`).
const GRIP_ICON_SIZE: f64 = 10.0;

/// Side of the lucide viewBox the grip dots' coordinates are authored in.
const ICON_VIEWBOX: f64 = 24.0;

/// The focus ring's width and outward offset, in logical px
/// (`focus-visible:ring-1 ring-offset-1`) — narrower than the catalog's
/// [`style::FOCUS_RING_WIDTH`], which is why this component paints its own.
const HANDLE_RING_WIDTH: f64 = 1.0;
/// The gap between the handle band and its ring (`ring-offset-1`).
const HANDLE_RING_OFFSET: f64 = 1.0;

/// Fraction differences below this count as "the same layout" — a guard against
/// reporting a no-op drag frame after frame.
const FRACTION_EPSILON: f64 = 1e-6;

/// The group's main axis. `Horizontal` = shadcn's default (`flex`), `Vertical`
/// is its `aria-[orientation=vertical]:flex-col`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResizableDirection {
    /// Panels side by side, seams running top-to-bottom.
    #[default]
    Horizontal,
    /// Panels stacked, seams running left-to-right.
    Vertical,
}

/// How every seam in a group is drawn — the `withHandle` prop, lifted to its
/// own spec (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResizableHandle {
    grip: bool,
}

/// A handle spec: a bare 1px seam by default, like the source's own
/// `withHandle`-less separator.
pub fn resizable_handle() -> ResizableHandle {
    ResizableHandle::default()
}

impl ResizableHandle {
    /// Show the bordered grip in the middle of every seam (`withHandle`).
    pub fn with_grip(mut self, grip: bool) -> Self {
        self.grip = grip;
        self
    }
}

/// One panel of a [`resizable_panel_group`]: a view plus its sizing bounds, all
/// fractions of the group's main axis.
pub struct ResizablePanel<State: 'static> {
    view: AnyView<State>,
    default_size: Option<f64>,
    min_size: f64,
    max_size: f64,
}

/// Wrap `view` as a resizable panel that takes whatever share the group's
/// arithmetic leaves it (`ResizablePrimitive.Panel`).
pub fn resizable_panel<State: 'static, V: View<State>>(view: V) -> ResizablePanel<State> {
    ResizablePanel {
        view: any(view),
        default_size: None,
        min_size: 0.0,
        max_size: 1.0,
    }
}

impl<State: 'static> ResizablePanel<State> {
    /// The panel's starting share of the group (`defaultSize / 100`). Panels
    /// without one split whatever the sized panels leave.
    pub fn default_size(mut self, fraction: f64) -> Self {
        self.default_size = Some(fraction.clamp(0.0, 1.0));
        self
    }

    /// The smallest share a drag may leave this panel (`minSize / 100`).
    pub fn min_size(mut self, fraction: f64) -> Self {
        self.min_size = fraction.clamp(0.0, 1.0);
        self
    }

    /// The largest share a drag may give this panel (`maxSize / 100`).
    pub fn max_size(mut self, fraction: f64) -> Self {
        self.max_size = fraction.clamp(0.0, 1.0);
        self
    }

    /// The bounds, ordered so an inverted pair cannot invert the clamp.
    fn bounds(&self) -> (f64, f64) {
        (
            self.min_size.min(self.max_size),
            self.max_size.max(self.min_size),
        )
    }
}

/// A view-held, typed layout callback (erased on build).
type OnLayout<State> = Rc<dyn Fn(&mut State, Vec<f64>)>;

/// A declarative shadcn resizable panel group. See the [module docs](self).
pub struct ResizablePanelGroupView<State: 'static> {
    panels: Vec<ResizablePanel<State>>,
    direction: ResizableDirection,
    handle: ResizableHandle,
    sizes: Option<Vec<f64>>,
    on_layout: Option<OnLayout<State>>,
}

/// Lay `panels` out along one axis with a draggable seam between each
/// neighbouring pair (`ResizablePrimitive.Group`), horizontally by default.
pub fn resizable_panel_group<State: 'static>(
    panels: Vec<ResizablePanel<State>>,
) -> ResizablePanelGroupView<State> {
    ResizablePanelGroupView {
        panels,
        direction: ResizableDirection::default(),
        handle: ResizableHandle::default(),
        sizes: None,
        on_layout: None,
    }
}

impl<State: 'static> ResizablePanelGroupView<State> {
    /// Set the group's axis (`direction`).
    pub fn direction(mut self, direction: ResizableDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Configure every seam (see [`resizable_handle`]).
    pub fn handle(mut self, handle: ResizableHandle) -> Self {
        self.handle = handle;
        self
    }

    /// Drive the layout from the app: the group paints exactly these fractions
    /// and reports every requested change through
    /// [`on_layout`](Self::on_layout) instead of applying it (see the
    /// [module docs](self)). A list whose length does not match the panels is
    /// ignored rather than half-applied.
    pub fn sizes(mut self, sizes: Vec<f64>) -> Self {
        self.sizes = Some(sizes);
        self
    }

    /// Report the group's fractions after every drag step or key nudge
    /// (`onLayout`).
    pub fn on_layout<F: Fn(&mut State, Vec<f64>) + 'static>(mut self, on_layout: F) -> Self {
        self.on_layout = Some(Rc::new(on_layout));
        self
    }

    /// The starting fractions: each panel's `default_size` where it has one,
    /// an equal share of what is left where it does not, normalized to sum to
    /// 1 (see the [module docs](self)).
    fn seed_fractions(&self) -> Vec<f64> {
        let count = self.panels.len();
        if count == 0 {
            return Vec::new();
        }
        let sized: f64 = self.panels.iter().filter_map(|p| p.default_size).sum();
        let unsized_count = self
            .panels
            .iter()
            .filter(|p| p.default_size.is_none())
            .count();
        let share = if unsized_count > 0 {
            (1.0 - sized).max(0.0) / unsized_count as f64
        } else {
            0.0
        };
        let raw: Vec<f64> = self
            .panels
            .iter()
            .map(|p| p.default_size.unwrap_or(share))
            .collect();
        normalize(raw, count)
    }

    /// The per-panel `(min, max)` bounds a drag clamps against.
    fn bounds(&self) -> Vec<(f64, f64)> {
        self.panels.iter().map(ResizablePanel::bounds).collect()
    }
}

/// Scale `raw` to sum to 1, falling back to an equal split when it sums to
/// nothing usable.
fn normalize(raw: Vec<f64>, count: usize) -> Vec<f64> {
    let total: f64 = raw.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return vec![1.0 / count as f64; count];
    }
    raw.into_iter().map(|f| f / total).collect()
}

impl<State: 'static> View<State> for ResizablePanelGroupView<State> {
    type Element = ResizablePanelGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ResizablePanelGroupWidget {
        let fractions = match &self.sizes {
            Some(sizes) if sizes.len() == self.panels.len() => {
                normalize(sizes.clone(), self.panels.len())
            }
            _ => self.seed_fractions(),
        };
        ResizablePanelGroupWidget {
            pods: self
                .panels
                .iter()
                .map(|p| build_child(&p.view, ctx))
                .collect(),
            fractions,
            bounds: self.bounds(),
            direction: self.direction,
            handle: self.handle,
            controlled: self.sizes.is_some(),
            main: 0.0,
            laid_out: Vec::new(),
            focused_handle: 0,
            captured: None,
            on_layout: self.on_layout.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ResizablePanelGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.panels.len() != self.panels.len() {
            // A changed panel count invalidates every recorded fraction; rebuild
            // the pod list and reseed from the new defaults.
            for (panel, pod) in prev.panels.iter().zip(element.pods.iter_mut()) {
                teardown_child(&panel.view, pod, ctx);
            }
            element.pods = self
                .panels
                .iter()
                .map(|p| build_child(&p.view, ctx))
                .collect();
            element.fractions = self.seed_fractions();
            element.captured = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for ((prev_panel, panel), pod) in prev
                .panels
                .iter()
                .zip(self.panels.iter())
                .zip(element.pods.iter_mut())
            {
                flags |= rebuild_child(&prev_panel.view, &panel.view, pod, ctx);
            }
        }

        element.bounds = self.bounds();
        element.controlled = self.sizes.is_some();
        if let Some(sizes) = &self.sizes
            && sizes.len() == element.pods.len()
        {
            // The app is the source of truth while controlled: adopt the
            // confirmed layout (this is what actually moves a handle mid-drag).
            let next = normalize(sizes.clone(), element.pods.len());
            if next != element.fractions {
                element.fractions = next;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        if element.direction != self.direction {
            element.direction = self.direction;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.handle != self.handle {
            element.handle = self.handle;
            flags |= ChangeFlags::PAINT;
        }
        element.on_layout = self.on_layout.as_ref().map(erase_callback_arg);
        element.focused_handle = element
            .focused_handle
            .min(element.pods.len().saturating_sub(2));
        flags
    }

    fn teardown(&self, element: &mut ResizablePanelGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (panel, pod) in self.panels.iter().zip(element.pods.iter_mut()) {
            teardown_child(&panel.view, pod, ctx);
        }
    }
}

/// The retained widget for a [`ResizablePanelGroupView`].
pub struct ResizablePanelGroupWidget {
    pods: Vec<ChildPod>,
    /// One fraction of the main axis per panel; always sums to 1.
    fractions: Vec<f64>,
    /// One `(min, max)` per panel.
    bounds: Vec<(f64, f64)>,
    direction: ResizableDirection,
    handle: ResizableHandle,
    /// Whether the app drives the layout (see the [module docs](self)).
    controlled: bool,
    /// The main-axis extent layout resolved, retained because the event pass
    /// maps a pointer delta back into fractions with it.
    main: f64,
    /// The fractions `layout` last placed the pods with — compared in `paint`
    /// so a drag that moved them can ask for the relayout `EventCtx` cannot.
    laid_out: Vec<f64>,
    /// The seam arrow keys move (the group is one tab stop, like a slider's
    /// roving thumb).
    focused_handle: usize,
    /// The seam a `Down` armed, plus the drag's start position and the
    /// fractions it started from, so every move is measured from the press
    /// rather than accumulated.
    captured: Option<Drag>,
    on_layout: Option<ErasedArgCallback<Vec<f64>>>,
}

/// A live handle drag.
struct Drag {
    handle: usize,
    /// The main-axis coordinate of the `Down`.
    start: f64,
    /// The fractions the drag started from.
    from: Vec<f64>,
}

impl ResizablePanelGroupWidget {
    /// The number of seams: one between each neighbouring pair of panels.
    fn handle_count(&self) -> usize {
        self.pods.len().saturating_sub(1)
    }

    /// The main-axis extent the panels actually share, once every seam has
    /// taken its 1px.
    fn available(&self) -> f64 {
        (self.main - self.handle_count() as f64 * RESIZABLE_HANDLE_THICKNESS).max(0.0)
    }

    /// The main-axis coordinate of seam `index`'s leading edge.
    fn handle_offset(&self, index: usize) -> f64 {
        let available = self.available();
        let mut at = 0.0;
        for i in 0..=index {
            at += self.fractions.get(i).copied().unwrap_or(0.0) * available;
            if i < index {
                at += RESIZABLE_HANDLE_THICKNESS;
            }
        }
        at
    }

    /// The seam whose hit band contains main-axis coordinate `pos`.
    fn handle_at(&self, pos: f64) -> Option<usize> {
        (0..self.handle_count()).find(|index| {
            let center = self.handle_offset(*index) + RESIZABLE_HANDLE_THICKNESS / 2.0;
            (pos - center).abs() <= RESIZABLE_HANDLE_HIT / 2.0
        })
    }

    /// A position's main-axis component under this group's direction.
    fn main_of(&self, point: Point) -> f64 {
        match self.direction {
            ResizableDirection::Horizontal => point.x,
            ResizableDirection::Vertical => point.y,
        }
    }

    /// The cursor a seam asks for under this group's direction.
    fn resize_cursor(&self) -> CursorIcon {
        match self.direction {
            ResizableDirection::Horizontal => CursorIcon::ColResize,
            ResizableDirection::Vertical => CursorIcon::RowResize,
        }
    }

    /// `from`, with `delta` of the main axis moved across seam `handle` —
    /// clamped so neither adjacent panel leaves its own `(min, max)`.
    fn redistributed(&self, from: &[f64], handle: usize, delta: f64) -> Vec<f64> {
        let mut next = from.to_vec();
        let (Some(&low), Some(&high)) = (from.get(handle), from.get(handle + 1)) else {
            return next;
        };
        let (low_min, low_max) = self.bounds.get(handle).copied().unwrap_or((0.0, 1.0));
        let (high_min, high_max) = self.bounds.get(handle + 1).copied().unwrap_or((0.0, 1.0));
        // The move is bounded on both sides at once: what the leading panel may
        // grow to, and what the trailing panel may shrink to.
        let lower = (low_min - low).max(high - high_max);
        let upper = (low_max - low).min(high - high_min);
        if lower > upper {
            // One of the two fractions already sits outside its own
            // `(min, max)` (author-supplied sizes can do this), so no `delta`
            // is legal — sorting `lower`/`upper` instead would silently admit
            // exactly the illegal range this bound exists to reject. Refuse
            // the move rather than legalize a size the app never asked for.
            return next;
        }
        let delta = delta.clamp(lower, upper);
        next[handle] = low + delta;
        next[handle + 1] = high - delta;
        next
    }

    /// Apply a requested layout: report it, and adopt it too unless the app is
    /// driving (see the [module docs](self)).
    ///
    /// The relayout itself is asked for from `paint`, not here — `EventCtx` has
    /// no layout request, so the next paint compares the adopted fractions
    /// against the ones `layout` last used and calls
    /// [`PaintCtx::request_layout`] when they have moved (the same route
    /// [`crate::components::collapsible`]'s reveal takes).
    fn request_sizes(&mut self, ctx: &mut EventCtx, next: Vec<f64>) -> bool {
        let unchanged = next
            .iter()
            .zip(self.fractions.iter())
            .all(|(a, b)| (a - b).abs() < FRACTION_EPSILON);
        if unchanged {
            return false;
        }
        if let Some(on_layout) = self.on_layout.as_mut() {
            on_layout(ctx, next.clone());
        }
        if !self.controlled {
            self.fractions = next;
        }
        ctx.request_redraw();
        true
    }

    /// Nudge the focused seam by `steps` key steps.
    fn nudge(&mut self, ctx: &mut EventCtx, steps: f64) -> bool {
        let handle = self.focused_handle;
        if handle >= self.handle_count() {
            return false;
        }
        let next = self.redistributed(&self.fractions.clone(), handle, steps * RESIZABLE_KEY_STEP);
        self.request_sizes(ctx, next)
    }

    /// The key steps an arrow means along this group's axis, if any.
    fn key_steps(&self, key: &Key) -> Option<f64> {
        match (self.direction, key) {
            (ResizableDirection::Horizontal, Key::Named(NamedKey::ArrowRight))
            | (ResizableDirection::Vertical, Key::Named(NamedKey::ArrowDown)) => Some(1.0),
            (ResizableDirection::Horizontal, Key::Named(NamedKey::ArrowLeft))
            | (ResizableDirection::Vertical, Key::Named(NamedKey::ArrowUp)) => Some(-1.0),
            _ => None,
        }
    }

    /// The seam's `(origin, size)` in widget-local coordinates.
    fn handle_rect(&self, index: usize, cross: f64) -> (Point, Size) {
        let at = self.handle_offset(index);
        match self.direction {
            ResizableDirection::Horizontal => (
                Point::new(at, 0.0),
                Size::new(RESIZABLE_HANDLE_THICKNESS, cross),
            ),
            ResizableDirection::Vertical => (
                Point::new(0.0, at),
                Size::new(cross, RESIZABLE_HANDLE_THICKNESS),
            ),
        }
    }
}

/// Paint lucide's `GripVerticalIcon` (two columns of three dots) centred on
/// `center`, `extent` px on a side, `swapped` for a vertical group's
/// quarter-turned grip.
fn draw_grip(scene: &mut dyn PaintScene, center: Point, extent: f64, swapped: bool, color: Color) {
    let scale = extent / ICON_VIEWBOX;
    // The six dot centres, viewBox-relative to its own centre (12, 12).
    let dots = [
        (-3.0, -7.0),
        (-3.0, 0.0),
        (-3.0, 7.0),
        (3.0, -7.0),
        (3.0, 0.0),
        (3.0, 7.0),
    ];
    // Lucide's dots are r=1 circles; a 2-unit square reads identically at this
    // size and costs no path flattening.
    let dot = 2.0 * scale;
    for (x, y) in dots {
        let (x, y) = if swapped { (y, x) } else { (x, y) };
        scene.fill_rounded_rect(
            Point::new(
                center.x + x * scale - dot / 2.0,
                center.y + y * scale - dot / 2.0,
            ),
            Size::new(dot, dot),
            dot / 2.0,
            color,
        );
    }
}

impl Widget for ResizablePanelGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        // `h-full w-full`: the group takes the space it is offered, falling back
        // to nothing on an unbounded axis (there is no intrinsic size in a
        // fraction-based layout).
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
        self.main = match self.direction {
            ResizableDirection::Horizontal => size.width,
            ResizableDirection::Vertical => size.height,
        };
        let cross = match self.direction {
            ResizableDirection::Horizontal => size.height,
            ResizableDirection::Vertical => size.width,
        };
        let available = self.available();

        let mut at = 0.0;
        for (index, pod) in self.pods.iter_mut().enumerate() {
            let extent = self.fractions.get(index).copied().unwrap_or(0.0) * available;
            let child_size = match self.direction {
                ResizableDirection::Horizontal => Size::new(extent, cross),
                ResizableDirection::Vertical => Size::new(cross, extent),
            };
            pod.layout_child(ctx, &BoxConstraints::tight(child_size));
            pod.set_origin(match self.direction {
                ResizableDirection::Horizontal => Point::new(at, 0.0),
                ResizableDirection::Vertical => Point::new(0.0, at),
            });
            at += extent + RESIZABLE_HANDLE_THICKNESS;
        }
        self.laid_out = self.fractions.clone();
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // A drag or a key nudge changed the fractions during the event pass,
        // which has no layout request of its own — this is where the relayout
        // is actually asked for.
        if self.laid_out != self.fractions {
            ctx.request_layout();
        }
        let origin = ctx.origin();
        let size = ctx.size();
        let focused = ctx.has_focus();
        let (border, ring) = {
            let theme = Theme::from_paint_ctx(ctx);
            let border = theme.map_or_else(
                || crate::tokens::color_scheme_light().outline_variant,
                |t| t.scheme().outline_variant,
            );
            (border, style::ring_color(None, theme))
        };
        let cross = match self.direction {
            ResizableDirection::Horizontal => size.height,
            ResizableDirection::Vertical => size.width,
        };

        for pod in &mut self.pods {
            pod.paint_child(ctx, scene);
        }

        let swapped = self.direction == ResizableDirection::Vertical;
        for index in 0..self.handle_count() {
            let (at, handle_size) = self.handle_rect(index, cross);
            let at = Point::new(origin.x + at.x, origin.y + at.y);
            scene.fill_rect(at, handle_size, border);

            if focused && index == self.focused_handle {
                // `ring-1 ring-offset-1`: a hairline ring one pixel out from the
                // hit band, not the catalog's 3px translucent one.
                let band = RESIZABLE_HANDLE_HIT / 2.0 + HANDLE_RING_OFFSET;
                let (dx, dy) = if swapped { (0.0, band) } else { (band, 0.0) };
                let outline = RoundedRect::from_rect(
                    Rect::from_origin_size(Point::ORIGIN, handle_size)
                        .inset((dx, dy))
                        .inset(HANDLE_RING_WIDTH / 2.0),
                    HANDLE_RING_WIDTH,
                );
                scene.stroke_path(
                    at,
                    &Shape::to_path(&outline, PATH_TOLERANCE),
                    HANDLE_RING_WIDTH,
                    &Brush::Solid(ring),
                );
            }

            if self.handle.grip {
                let grip = if swapped {
                    Size::new(GRIP_SIZE.height, GRIP_SIZE.width)
                } else {
                    GRIP_SIZE
                };
                let center = Point::new(
                    at.x + handle_size.width / 2.0,
                    at.y + handle_size.height / 2.0,
                );
                let grip_origin =
                    Point::new(center.x - grip.width / 2.0, center.y - grip.height / 2.0);
                scene.fill_rounded_rect(grip_origin, grip, GRIP_RADIUS, border);
                // The dots read as the foreground would over that fill; the
                // source leaves them `currentColor`, and the fill is the border
                // token, so they take the surface behind it.
                draw_grip(scene, center, GRIP_ICON_SIZE, swapped, ring);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return route_event(&mut self.pods, ctx, event);
        }
        let size = ctx.size();

        // A live drag owns every pointer pass until it ends — the panels never
        // see one, so a control inside a panel cannot steal a resize.
        if let Some(drag) = &self.captured
            && let InputEvent::Pointer(p) = event
        {
            let (handle, start, from) = (drag.handle, drag.start, drag.from.clone());
            return match p.phase {
                PointerPhase::Move => {
                    // The captured arm re-asks every move, which is what keeps
                    // the resize cursor alive outside the group's bounds.
                    ctx.set_cursor(self.resize_cursor());
                    let delta = (self.main_of(p.position) - start) / self.available().max(1.0);
                    let next = self.redistributed(&from, handle, delta);
                    self.request_sizes(ctx, next);
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    self.captured = None;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // Internal flags only — a cancel keeps whatever layout the
                    // last move settled on.
                    self.captured = None;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Down => EventResult::Handled,
            };
        }

        let routed = route_event(&mut self.pods, ctx, event);
        if routed == EventResult::Handled {
            return routed;
        }

        match event {
            InputEvent::Key(key) => {
                let Some(steps) = self.key_steps(&key.key) else {
                    return EventResult::Ignored;
                };
                self.nudge(ctx, steps);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let main = self.main_of(p.position);
                match p.phase {
                    PointerPhase::Down => {
                        if !presses(p) {
                            return EventResult::Ignored;
                        }
                        let Some(handle) = self.handle_at(main) else {
                            return EventResult::Ignored;
                        };
                        self.captured = Some(Drag {
                            handle,
                            start: main,
                            from: self.fractions.clone(),
                        });
                        self.focused_handle = handle;
                        ctx.capture_pointer();
                        // Focus is what paints the seam's ring and routes the
                        // arrow keys here.
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        // The hover/cursor pass, claimed *after* routing so a
                        // panel's own claim wins where the two overlap.
                        if crate::hit::inside(p.position, size) {
                            ctx.claim_hover();
                            if self.handle_at(main).is_some() {
                                ctx.set_cursor(self.resize_cursor());
                            }
                        }
                        EventResult::Ignored
                    }
                    PointerPhase::Up | PointerPhase::Cancel => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                for (index, pod) in self.pods.iter().enumerate() {
                    pod.semantics_child(ctx);
                    if index < self.handle_count() {
                        ctx.push_node(Role::Splitter, |node| {
                            // The seam's own position, so a screen reader can
                            // announce what a nudge actually moved.
                            node.set_numeric_value(self.fractions[index]);
                            node.add_action(Action::Focus);
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
        BezPath, EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    const GROUP: Size = Size::new(400.0, 200.0);

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
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
    }

    #[derive(Default)]
    struct Layouts {
        sizes: Vec<f64>,
        reports: Vec<Vec<f64>>,
    }

    /// A fixed-size leaf generic over the app state (the shared
    /// `test_support::leaf` is `View<()>` only).
    struct Block;

    /// The retained half of [`Block`].
    struct BlockWidget;

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    fn two_panels() -> ResizablePanelGroupView<Layouts> {
        resizable_panel_group(vec![
            resizable_panel(Block).default_size(0.5).min_size(0.2),
            resizable_panel(Block).default_size(0.5).min_size(0.2),
        ])
        .on_layout(|s: &mut Layouts, sizes: Vec<f64>| {
            s.sizes = sizes.clone();
            s.reports.push(sizes);
        })
    }

    fn build(v: &ResizablePanelGroupView<Layouts>) -> ResizablePanelGroupWidget {
        let mut counter = 0u64;
        View::<Layouts>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn laid_out(v: &ResizablePanelGroupView<Layouts>) -> (ResizablePanelGroupWidget, Size) {
        laid_out_in(v, GROUP)
    }

    fn laid_out_in(
        v: &ResizablePanelGroupView<Layouts>,
        max: Size,
    ) -> (ResizablePanelGroupWidget, Size) {
        let mut w = build(v);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(max));
        (w, size)
    }

    fn paint(w: &mut ResizablePanelGroupWidget, size: Size, theme: &Theme) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, size).with_theme(theme);
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
        w: &mut ResizablePanelGroupWidget,
        state: &mut Layouts,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    // ---- Layout ------------------------------------------------------------

    #[test]
    fn panels_split_the_axis_by_fraction_around_a_one_pixel_seam() {
        let (w, size) = laid_out(&two_panels());
        assert_eq!(size, GROUP);
        let available = GROUP.width - RESIZABLE_HANDLE_THICKNESS;
        assert_eq!(w.pods[0].origin(), Point::ZERO);
        assert_eq!(
            w.pods[1].origin().x,
            available / 2.0 + RESIZABLE_HANDLE_THICKNESS
        );
        assert_eq!(w.handle_offset(0), available / 2.0);
    }

    #[test]
    fn a_vertical_group_stacks_its_panels() {
        let (w, _) = laid_out(&two_panels().direction(ResizableDirection::Vertical));
        let available = GROUP.height - RESIZABLE_HANDLE_THICKNESS;
        assert_eq!(w.pods[0].origin(), Point::ZERO);
        assert_eq!(
            w.pods[1].origin().y,
            available / 2.0 + RESIZABLE_HANDLE_THICKNESS
        );
    }

    #[test]
    fn unsized_panels_split_what_the_sized_ones_leave() {
        let view: ResizablePanelGroupView<Layouts> = resizable_panel_group(vec![
            resizable_panel(Block).default_size(0.5),
            resizable_panel(Block),
            resizable_panel(Block),
        ]);
        let (w, _) = laid_out(&view);
        assert_eq!(w.fractions, vec![0.5, 0.25, 0.25]);

        // Defaults that do not add up are normalized rather than obeyed.
        let over: ResizablePanelGroupView<Layouts> = resizable_panel_group(vec![
            resizable_panel(Block).default_size(0.8),
            resizable_panel(Block).default_size(0.8),
        ]);
        let (w, _) = laid_out(&over);
        assert_eq!(w.fractions, vec![0.5, 0.5]);
    }

    #[test]
    fn a_tight_group_and_an_unbounded_one_both_lay_out() {
        // Tight enough that the seam is most of the axis: extents clamp at zero
        // rather than going negative.
        let (tight, size) = laid_out_in(&two_panels(), Size::new(1.0, 10.0));
        assert_eq!(size, Size::new(1.0, 10.0));
        assert_eq!(tight.available(), 0.0);
        assert_eq!(tight.pods[1].origin().x, RESIZABLE_HANDLE_THICKNESS);

        // Unbounded: a fraction layout has no intrinsic size to fall back on.
        let (loose, size) = laid_out_in(&two_panels(), Size::new(f64::INFINITY, 200.0));
        assert_eq!(size.width, 0.0);
        assert_eq!(loose.available(), 0.0);
    }

    // ---- Drag --------------------------------------------------------------

    #[test]
    fn dragging_the_handle_redistributes_the_two_adjacent_panels() {
        let (mut w, size) = laid_out(&two_panels());
        let mut state = Layouts::default();
        let seam = w.handle_offset(0);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, seam, 100.0),
        );
        assert!(w.captured.is_some());
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seam + 40.0, 100.0),
        );
        let available = size.width - RESIZABLE_HANDLE_THICKNESS;
        let moved = 40.0 / available;
        let reported = state.sizes.clone();
        assert!((reported[0] - (0.5 + moved)).abs() < 1e-9);
        assert!((reported[1] - (0.5 - moved)).abs() < 1e-9);
        assert!(
            (reported.iter().sum::<f64>() - 1.0).abs() < 1e-9,
            "a drag never changes the total"
        );
        // Uncontrolled: the widget applies what it reported.
        assert_eq!(w.fractions, reported);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, seam + 40.0, 100.0),
        );
        assert!(w.captured.is_none());
    }

    #[test]
    fn a_drag_is_clamped_by_both_panels_bounds() {
        let (mut w, size) = laid_out(&two_panels());
        let mut state = Layouts::default();
        let seam = w.handle_offset(0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, seam, 100.0),
        );
        // Far past the end: the trailing panel's `min_size` stops it.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seam + 10_000.0, 100.0),
        );
        assert!((state.sizes[1] - 0.2).abs() < 1e-9, "min_size holds");
        assert!((state.sizes[0] - 0.8).abs() < 1e-9);

        // ...and the other way, against the leading panel's own minimum.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seam - 10_000.0, 100.0),
        );
        assert!((state.sizes[0] - 0.2).abs() < 1e-9);
    }

    #[test]
    fn a_max_size_caps_the_growing_panel() {
        let view: ResizablePanelGroupView<Layouts> = resizable_panel_group(vec![
            resizable_panel(Block).default_size(0.5).max_size(0.6),
            resizable_panel(Block).default_size(0.5),
        ])
        .on_layout(|s: &mut Layouts, sizes: Vec<f64>| s.sizes = sizes);
        let (mut w, size) = laid_out(&view);
        let mut state = Layouts::default();
        let seam = w.handle_offset(0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, seam, 100.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seam + 10_000.0, 100.0),
        );
        assert!((state.sizes[0] - 0.6).abs() < 1e-9, "max_size holds");
    }

    #[test]
    fn contradictory_author_bounds_refuse_every_move_instead_of_legalizing_one() {
        // Both panels cap at 0.4, but a two-panel group's fractions always sum
        // to 1 — the author's own bounds are jointly infeasible, so the seed
        // already leaves each panel's fraction (0.5) above its own `max_size`.
        // A drag from here must refuse rather than accept a `delta` that
        // sorting `(lower, upper)` would otherwise silently legalize.
        let view: ResizablePanelGroupView<Layouts> = resizable_panel_group(vec![
            resizable_panel(Block).max_size(0.4),
            resizable_panel(Block).max_size(0.4),
        ])
        .on_layout(|s: &mut Layouts, sizes: Vec<f64>| {
            s.sizes = sizes.clone();
            s.reports.push(sizes);
        });
        let (mut w, size) = laid_out(&view);
        assert_eq!(
            w.fractions,
            vec![0.5, 0.5],
            "the seed already violates max_size"
        );
        let seam = w.handle_offset(0);
        let mut state = Layouts::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, seam, 100.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seam + 20.0, 100.0),
        );
        assert_eq!(
            w.fractions,
            vec![0.5, 0.5],
            "the move is refused, not partly admitted"
        );
        assert!(
            state.reports.is_empty(),
            "no layout is reported when the move was refused"
        );
    }

    #[test]
    fn a_cancelled_drag_disarms_and_a_still_pointer_reports_nothing() {
        let (mut w, size) = laid_out(&two_panels());
        let mut state = Layouts::default();
        let seam = w.handle_offset(0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, seam, 100.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seam, 100.0),
        );
        assert!(
            state.reports.is_empty(),
            "a drag that moved nothing is silent"
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, seam + 30.0, 100.0),
        );
        assert!(w.captured.is_none());
        assert!(
            state.reports.is_empty(),
            "a cancel reports nothing of its own"
        );
    }

    #[test]
    fn a_press_off_every_seam_is_not_a_resize() {
        let (mut w, size) = laid_out(&two_panels());
        let mut state = Layouts::default();
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(PointerPhase::Down, 20.0, 100.0)
            ),
            EventResult::Ignored
        );
        assert!(w.captured.is_none());
    }

    // ---- Keyboard, controlled mode -----------------------------------------

    #[test]
    fn arrow_keys_nudge_the_focused_handle_along_the_axis() {
        let (mut w, size) = laid_out(&two_panels());
        let mut state = Layouts::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert!((state.sizes[0] - (0.5 + RESIZABLE_KEY_STEP)).abs() < 1e-9);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert!((state.sizes[0] - 0.5).abs() < 1e-9);
        // The cross-axis arrows belong to somebody else.
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &key(Key::Named(NamedKey::ArrowDown))
            ),
            EventResult::Ignored
        );

        let (mut vertical, size) = laid_out(&two_panels().direction(ResizableDirection::Vertical));
        dispatch(
            &mut vertical,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowDown)),
        );
        assert!((state.sizes[0] - (0.5 + RESIZABLE_KEY_STEP)).abs() < 1e-9);
        assert_eq!(
            dispatch(
                &mut vertical,
                &mut state,
                size,
                &key(Key::Named(NamedKey::ArrowRight))
            ),
            EventResult::Ignored
        );
    }

    #[test]
    fn a_controlled_group_reports_without_applying_until_the_app_confirms() {
        let controlled = || {
            resizable_panel_group(vec![
                resizable_panel(Block).default_size(0.5),
                resizable_panel(Block).default_size(0.5),
            ])
            .sizes(vec![0.5, 0.5])
            .on_layout(|s: &mut Layouts, sizes: Vec<f64>| s.sizes = sizes)
        };
        let prev = controlled();
        let (mut w, size) = laid_out(&prev);
        let mut state = Layouts::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert!(
            (state.sizes[0] - 0.55).abs() < 1e-9,
            "the request is reported"
        );
        assert_eq!(w.fractions, vec![0.5, 0.5], "the app owns the layout");

        // Once it confirms, the widget adopts it.
        let next = resizable_panel_group(vec![
            resizable_panel(Block).default_size(0.5),
            resizable_panel(Block).default_size(0.5),
        ])
        .sizes(vec![0.55, 0.45])
        .on_layout(|s: &mut Layouts, sizes: Vec<f64>| s.sizes = sizes);
        let mut counter = 0u64;
        let flags =
            View::<Layouts>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!((w.fractions[0] - 0.55).abs() < 1e-9);
        assert!(flags.needs_layout());
    }

    #[test]
    fn a_changed_panel_count_reseeds_the_layout() {
        let prev = two_panels();
        let (mut w, _) = laid_out(&prev);
        w.fractions = vec![0.8, 0.2];
        let next: ResizablePanelGroupView<Layouts> = resizable_panel_group(vec![
            resizable_panel(Block),
            resizable_panel(Block),
            resizable_panel(Block),
        ]);
        let mut counter = 0u64;
        View::<Layouts>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.pods.len(), 3);
        assert!(w.fractions.iter().all(|f| (f - 1.0 / 3.0).abs() < 1e-9));
    }

    // ---- Paint -------------------------------------------------------------

    #[test]
    fn each_seam_paints_a_hairline_and_the_grip_is_opt_in() {
        let theme = crate::theme();
        let (mut bare, size) = laid_out(&two_panels());
        let rec = paint(&mut bare, size, &theme);
        assert_eq!(rec.rects.len(), 1, "one seam, one hairline");
        let (at, seam_size, color) = rec.rects[0];
        assert_eq!(
            seam_size,
            Size::new(RESIZABLE_HANDLE_THICKNESS, GROUP.height)
        );
        assert_eq!(at.x, bare.handle_offset(0));
        assert_eq!(color, theme.scheme().outline_variant);
        assert!(rec.rrects.is_empty(), "no grip without `withHandle`");

        let (mut gripped, size) =
            laid_out(&two_panels().handle(resizable_handle().with_grip(true)));
        let rec = paint(&mut gripped, size, &theme);
        assert_eq!(rec.rrects.len(), 1 + 6, "the grip plus its six dots");
        let (grip_origin, grip_size, radius, _) = rec.rrects[0];
        assert_eq!(grip_size, GRIP_SIZE);
        assert_eq!(radius, GRIP_RADIUS);
        assert!((grip_origin.y + grip_size.height / 2.0 - GROUP.height / 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_vertical_groups_grip_is_quarter_turned() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(
            &two_panels()
                .direction(ResizableDirection::Vertical)
                .handle(resizable_handle().with_grip(true)),
        );
        let rec = paint(&mut w, size, &theme);
        let (_, seam_size, _) = rec.rects[0];
        assert_eq!(
            seam_size,
            Size::new(GROUP.width, RESIZABLE_HANDLE_THICKNESS)
        );
        let (_, grip_size, _, _) = rec.rrects[0];
        assert_eq!(grip_size, Size::new(GRIP_SIZE.height, GRIP_SIZE.width));
        // The dot column runs across the group instead of along it.
        let dots: Vec<Point> = rec.rrects[1..].iter().map(|(o, _, _, _)| *o).collect();
        let spread_x = dots.iter().map(|p| p.x).fold(f64::MIN, f64::max)
            - dots.iter().map(|p| p.x).fold(f64::MAX, f64::min);
        let spread_y = dots.iter().map(|p| p.y).fold(f64::MIN, f64::max)
            - dots.iter().map(|p| p.y).fold(f64::MAX, f64::min);
        assert!(spread_x > spread_y);
    }

    // ---- Root-driven: cursor, focus ring, semantics --------------------------

    struct Harness {
        root: RenderRoot<Layouts, ResizablePanelGroupView<Layouts>>,
        state: Layouts,
    }

    impl Harness {
        fn new(direction: ResizableDirection) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: Layouts::default(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Layouts| {
                resizable_panel_group(vec![
                    resizable_panel(Block).default_size(0.5),
                    resizable_panel(Block).default_size(0.5),
                ])
                .direction(direction)
            };
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout(GROUP);
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

    #[test]
    fn the_cursor_is_the_axis_resize_shape_over_a_seam_and_during_a_drag() {
        let seam = (GROUP.width - RESIZABLE_HANDLE_THICKNESS) / 2.0;
        let mut h = Harness::new(ResizableDirection::Horizontal);
        h.dispatch(&pointer(PointerPhase::Move, seam, 100.0));
        assert_eq!(h.root.cursor(), CursorIcon::ColResize);

        // Off the seam, the group asks for no shape of its own.
        h.dispatch(&pointer(PointerPhase::Move, 20.0, 100.0));
        assert_eq!(h.root.cursor(), CursorIcon::Default);

        // A captured drag keeps the shape well outside the group.
        h.dispatch(&pointer(PointerPhase::Down, seam, 100.0));
        h.dispatch(&pointer(PointerPhase::Move, GROUP.width + 500.0, 900.0));
        assert_eq!(h.root.cursor(), CursorIcon::ColResize);

        let vseam = (GROUP.height - RESIZABLE_HANDLE_THICKNESS) / 2.0;
        let mut vertical = Harness::new(ResizableDirection::Vertical);
        vertical.dispatch(&pointer(PointerPhase::Move, 100.0, vseam));
        assert_eq!(vertical.root.cursor(), CursorIcon::RowResize);
    }

    #[test]
    fn a_press_focuses_the_seam_and_the_next_paint_rings_it() {
        let mut h = Harness::new(ResizableDirection::Horizontal);
        assert!(h.frame().strokes.is_empty(), "no ring at rest");
        let seam = (GROUP.width - RESIZABLE_HANDLE_THICKNESS) / 2.0;
        h.dispatch(&pointer(PointerPhase::Down, seam, 100.0));
        assert!(h.root.is_focus_active());
        h.dispatch(&pointer(PointerPhase::Up, seam, 100.0));

        let rec = h.frame();
        assert_eq!(rec.strokes.len(), 1, "one hairline ring");
        let (bbox, width, color) = rec.strokes[0];
        assert_eq!(width, HANDLE_RING_WIDTH);
        assert_eq!(color, style::ring_color(None, Some(&crate::theme())));
        assert!(
            bbox.x0 < seam - RESIZABLE_HANDLE_HIT / 2.0,
            "the ring sits outside the hit band"
        );
    }

    #[test]
    fn semantics_reports_a_splitter_between_every_pair_of_panels() {
        let h = Harness::new(ResizableDirection::Horizontal);
        let update = h.semantics();
        let splitters: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Splitter)
            .collect();
        assert_eq!(splitters.len(), 1, "two panels, one seam");
        assert_eq!(splitters[0].1.numeric_value(), Some(0.5));
        assert!(splitters[0].1.supports_action(Action::Focus));
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Group),
            "the group wraps its panels"
        );
    }

    #[test]
    fn visit_children_publishes_every_panel() {
        let w = build(&two_panels());
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 2);
    }
}
