//! Ports beUI's `bloom-menu` block — `components/motion/bloom-menu.tsx` (beUI
//! rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `bloom-menu`: *"A button that morphs open into a menu and
//! blooms iris-out from the center, the grid revealing in every direction with
//! radially staggered items."*
//!
//! | upstream | here |
//! |---|---|
//! | trigger `h-11 w-36 rounded-[16px] border border-border bg-card text-sm font-medium` | [`BLOOM_TRIGGER_WIDTH`], [`BLOOM_TRIGGER_HEIGHT`], [`BLOOM_RADIUS`] |
//! | centring box `h-[300px] w-[min(86vw,420px)]` | [`BLOOM_BOX_HEIGHT`], [`BLOOM_PANEL_MAX_WIDTH`], [`BLOOM_PANEL_WIDTH_FRACTION`] |
//! | panel `w-[min(86vw,420px)] overflow-hidden border border-border bg-card`, `borderRadius: 16` | the same box, the shared panel chrome |
//! | `SPRING_FOLDER` `{stiffness 300, damping 32, mass 0.9}` | [`BLOOM_FOLDER`] |
//! | header `justify-between border-b border-border px-4 py-3`, `text-sm font-medium text-muted-foreground` | [`BLOOM_HEADER_HEIGHT`], [`BLOOM_HEADER_PADDING_X`] |
//! | content fade `{delay: 0.12, duration: 0.2}` | [`BLOOM_CONTENT_DELAY`], [`BLOOM_CONTENT_FADE`] |
//! | iris `inset(45% 34% 45% 34%)` → `inset(0)`, `{delay: 0.08, duration: 0.45, EASE_OUT}` | [`bloom_iris_rect`], [`BLOOM_IRIS_DELAY`], [`BLOOM_IRIS`] |
//! | grid `grid-cols-3`, cell `px-3 py-6`, hairlines `border-r`/`border-b` | [`BLOOM_COLUMNS`], [`bloom_cell_rect`] |
//! | cell content `gap-2`, icon `h-5 w-5`, label `text-sm font-medium` | [`BLOOM_CELL_GAP`], [`BLOOM_ICON_SIZE`] |
//! | radial delay `0.1 + dist * 0.07`, `dist = hypot(col − (cols−1)/2, row − (rows−1)/2)` | [`bloom_radial_distance`], [`bloom_item_delay`] |
//! | item `{opacity, scale: 0.85, blur(6px)}` on `{stiffness 440, damping 34}` | [`BLOOM_ITEM`], [`BLOOM_ITEM_SCALE`] |
//! | `whileTap: {scale: 0.97}` on the trigger | [`style::PRESS_SCALE_CSS`] |
//!
//! # A premise correction: this is a grid, not an arc
//!
//! The porting card describes a *"radial 'bloom' menu: actions bloom outward
//! from a trigger along arcs"*. Upstream is not that. `bloom-menu.tsx` is a
//! **three-column card grid** that irises out from its own centre, and the
//! "radial" part is purely the **timing**: each cell's delay is its own
//! Euclidean distance from the grid's centre, so *"the four corners animate
//! together and the open reads as center-out, not corner-by-corner"*. Nothing
//! is placed on an arc; nothing travels along one. The port follows the source,
//! and [`bloom_radial_distance`] is where the radial part actually lives.
//!
//! # It grows in place, and it is not portalled
//!
//! Unlike every other overlay in this catalog, `bloom-menu` calls no
//! `createPortal`: the panel is an absolutely-positioned sibling inside a
//! **fixed-size centring box** the trigger sits in the middle of, so the page
//! around it never reflows as the menu opens. That is why this block mounts
//! through neither [`crate::overlay::modal`] nor
//! [`crate::overlay::anchored`] — there is nothing to host. The widget *is*
//! the centring box: [`BLOOM_BOX_HEIGHT`] tall and as wide as the panel, with
//! the trigger centred in it, so its own layout box never changes size while
//! the menu opens.
//!
//! The morph itself is one [`Lane`](crate::press::Lane) between the trigger's
//! box and the panel's, both centred on the same point — upstream's shared
//! `layoutId` between the two, which is a size change about a fixed centre and
//! nothing else.
//!
//! # Degradations against the web original
//!
//! - **The outside press only reaches the centring box.** Upstream listens for
//!   `pointerdown` on `window`; a widget here sees only what is routed to it, so
//!   a press inside the box but outside the panel dismisses, and a press
//!   elsewhere on screen is the app's business. Escape (once the menu holds
//!   focus) and the header's close mark are the other two ways out.
//! - **No blur on the item reveal.** `filter: blur(6px)` has no `PaintScene`
//!   primitive; the opacity and the `scale: 0.85` halves are kept.
//! - **The icons are caller-supplied views**, the child protocol
//!   [`crate::components::dock`] established: the catalog ships no icon set, so
//!   upstream's lucide marks are the app's to provide. The trigger's own `Plus`
//!   is drawn ([`draw_plus`]), as is the header's `X`, because those two are the
//!   component's own chrome rather than an item's.
//! - **Keyboard selection is an addition.** Upstream's cells are focusable
//!   `<button>`s and it ships no arrow handling; the whole menu is one widget
//!   here, so the arrows walk a menu-local highlight across the grid and Enter
//!   commits it — the call [`crate::components::context_menu`] makes for the
//!   same reason. The visible effect is upstream's own `hover:text-foreground`,
//!   which is what the highlight paints.
//! - **`reduce_motion` opens on the spot.** Upstream's reduced branch keeps a
//!   `0.15s` morph and drops every delay; here the lane snaps and the iris,
//!   fade and radial delays all collapse, which is the catalog-wide reduced
//!   posture.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod,
    Color, ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2,
    View, Widget, any, build_child, erase_callback_arg, rebuild_children, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::components::popover::{PanelChrome, lerp, paint_panel, resolve_panel};
use crate::motion::Ramp;
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// `w-36` — the closed trigger's width, in logical px.
pub const BLOOM_TRIGGER_WIDTH: f64 = 144.0;

/// `h-11` — the closed trigger's height, in logical px.
pub const BLOOM_TRIGGER_HEIGHT: f64 = 44.0;

/// `borderRadius: 16` — the radius the trigger and the panel share, held
/// constant through the morph, in logical px.
pub const BLOOM_RADIUS: f64 = style::RADIUS_2XL;

/// `min(…, 420px)` — the panel's width cap, in logical px.
pub const BLOOM_PANEL_MAX_WIDTH: f64 = 420.0;

/// `min(86vw, …)` — the share of the available width the panel takes below that
/// cap.
pub const BLOOM_PANEL_WIDTH_FRACTION: f64 = 0.86;

/// `h-[300px]` — the centring box's height, in logical px. The widget's own box
/// is this tall whether the menu is open or shut.
pub const BLOOM_BOX_HEIGHT: f64 = 300.0;

/// `grid-cols-3` — the grid's column count.
pub const BLOOM_COLUMNS: usize = 3;

/// `px-3` — a cell's horizontal padding, in logical px.
pub const BLOOM_CELL_PADDING_X: f64 = 12.0;

/// `py-6` — a cell's vertical padding, in logical px.
pub const BLOOM_CELL_PADDING_Y: f64 = 24.0;

/// `gap-2` — the gap between a cell's icon and its label, in logical px.
pub const BLOOM_CELL_GAP: f64 = 8.0;

/// `h-5 w-5` — a cell icon's box, in logical px.
pub const BLOOM_ICON_SIZE: f64 = 20.0;

/// One grid cell's height, in logical px: `py-6` around an icon, a gap and a
/// `text-sm` line.
pub const BLOOM_CELL_HEIGHT: f64 =
    BLOOM_CELL_PADDING_Y * 2.0 + BLOOM_ICON_SIZE + BLOOM_CELL_GAP + 20.0;

/// `px-4` — the header's horizontal padding, in logical px.
pub const BLOOM_HEADER_PADDING_X: f64 = 16.0;

/// `py-3` — the header's vertical padding, in logical px.
pub const BLOOM_HEADER_PADDING_Y: f64 = 12.0;

/// The header's height, in logical px (`py-3` around a `text-sm` line).
pub const BLOOM_HEADER_HEIGHT: f64 = BLOOM_HEADER_PADDING_Y * 2.0 + 20.0;

/// `h-4 w-4` — the header's close mark, in logical px.
pub const BLOOM_CLOSE_ICON: f64 = 16.0;

/// `h-4 w-4` — the trigger's `Plus` mark, in logical px.
pub const BLOOM_PLUS_ICON: f64 = 16.0;

/// The gap between the trigger's label and its `Plus` mark, in logical px
/// (`gap-2`).
pub const BLOOM_TRIGGER_GAP: f64 = 8.0;

/// The vertical share of the grid the folded iris still shows on each side
/// (`inset(45% …)`).
pub const BLOOM_IRIS_INSET_Y: f64 = 0.45;

/// The horizontal share (`inset(… 34% …)`).
pub const BLOOM_IRIS_INSET_X: f64 = 0.34;

/// `scale: 0.85` — the scale a cell's content blooms from.
pub const BLOOM_ITEM_SCALE: f64 = 0.85;

// ---- Motion ----------------------------------------------------------------

/// `SPRING_FOLDER` — the trigger↔panel morph, *"a touch of overshoot as the
/// panel expands, kept subtle"*.
pub const BLOOM_FOLDER: SpringDescription = SpringDescription {
    mass: 0.9,
    stiffness: 300.0,
    damping: 32.0,
};

/// The per-cell bloom spring (`{type: "spring", stiffness: 440, damping: 34}`,
/// Motion's default unit mass).
pub const BLOOM_ITEM: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 440.0,
    damping: 34.0,
};

/// How long the iris takes to open (`duration: 0.45`).
pub const BLOOM_IRIS: Duration = Duration::from_millis(450);

/// How long it waits first (`delay: 0.08`).
pub const BLOOM_IRIS_DELAY: Duration = Duration::from_millis(80);

/// How long the panel's contents take to fade in (`duration: 0.2`).
pub const BLOOM_CONTENT_FADE: Duration = Duration::from_millis(200);

/// How long that fade waits first (`delay: 0.12`).
pub const BLOOM_CONTENT_DELAY: Duration = Duration::from_millis(120);

/// The delay every cell carries before its own distance is added
/// (`delay: 0.1 + …`).
pub const BLOOM_ITEM_BASE_DELAY: Duration = Duration::from_millis(100);

/// How much delay one unit of radial distance adds (`… + dist * 0.07`).
pub const BLOOM_ITEM_DELAY_PER_UNIT: Duration = Duration::from_millis(70);

// ---- The bloom arithmetic --------------------------------------------------

/// How many rows a `count`-item grid of `columns` columns needs.
///
/// `Math.ceil(items.length / cols)`. A zero column count reports no rows rather
/// than dividing by it.
pub fn bloom_grid_rows(count: usize, columns: usize) -> usize {
    if columns == 0 || count == 0 {
        return 0;
    }
    count.div_ceil(columns)
}

/// Item `index`'s cell inside `grid`, in `grid`'s own space.
///
/// Row-major over `columns` equal columns of [`BLOOM_CELL_HEIGHT`], which is
/// what a CSS `grid-cols-3` of fixed-height cells resolves to. An index outside
/// the grid reports an empty rect at the grid's origin.
pub fn bloom_cell_rect(index: usize, count: usize, columns: usize, grid: Rect) -> Rect {
    if columns == 0 || index >= count {
        return Rect::from_origin_size(grid.origin(), Size::ZERO);
    }
    let column = index % columns;
    let row = index / columns;
    // The trailing column takes the rounding remainder, so the cells tile the
    // grid exactly rather than leaving a sub-pixel gutter at the right edge.
    let width = grid.width() / columns as f64;
    let x0 = grid.x0 + width * column as f64;
    let x1 = if column + 1 == columns {
        grid.x1
    } else {
        x0 + width
    };
    let y0 = grid.y0 + BLOOM_CELL_HEIGHT * row as f64;
    Rect::new(x0, y0, x1, y0 + BLOOM_CELL_HEIGHT)
}

/// Item `index`'s distance from the grid's centre, in cells.
///
/// `Math.hypot(col − (cols − 1) / 2, row − (rows − 1) / 2)` — the radial part of
/// the bloom, and the whole reason the corners arrive together.
pub fn bloom_radial_distance(index: usize, count: usize, columns: usize) -> f64 {
    let rows = bloom_grid_rows(count, columns);
    if columns == 0 || rows == 0 || index >= count {
        return 0.0;
    }
    let column = (index % columns) as f64;
    let row = (index / columns) as f64;
    let cx = (columns as f64 - 1.0) / 2.0;
    let cy = (rows as f64 - 1.0) / 2.0;
    (column - cx).hypot(row - cy)
}

/// How long after the bloom starts item `index` begins moving:
/// [`BLOOM_ITEM_BASE_DELAY`] plus its [`bloom_radial_distance`] scaled by
/// [`BLOOM_ITEM_DELAY_PER_UNIT`].
pub fn bloom_item_delay(index: usize, count: usize, columns: usize) -> Duration {
    BLOOM_ITEM_BASE_DELAY
        + BLOOM_ITEM_DELAY_PER_UNIT.mul_f64(bloom_radial_distance(index, count, columns))
}

/// The iris rect over `grid` at `progress`: `inset(45% 34% 45% 34%)` folded,
/// `inset(0)` open.
///
/// `progress` is clamped, so an overshooting driver cannot tear the clip off
/// the grid it belongs to.
pub fn bloom_iris_rect(grid: Rect, progress: f64) -> Rect {
    let t = progress.clamp(0.0, 1.0);
    let x = grid.width() * BLOOM_IRIS_INSET_X * (1.0 - t);
    let y = grid.height() * BLOOM_IRIS_INSET_Y * (1.0 - t);
    Rect::new(grid.x0 + x, grid.y0 + y, grid.x1 - x, grid.y1 - y)
}

/// How present item `index` is at `elapsed` into the bloom, in `[0, 1]`.
///
/// Zero until its own [`bloom_item_delay`] has passed, then its own
/// [`BLOOM_ITEM`] spring, clamped. `reduce` collapses every delay and the spring
/// with them.
pub fn bloom_item_reveal(elapsed: Duration, index: usize, count: usize, reduce: bool) -> f64 {
    if reduce {
        return 1.0;
    }
    let delay = bloom_item_delay(index, count, BLOOM_COLUMNS);
    let Some(since) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    Ramp::spring(BLOOM_ITEM).progress_clamped(since)
}

// ---- Items -----------------------------------------------------------------

/// One cell of the bloom grid: an icon view and its label.
pub struct BloomMenuItem<State: 'static> {
    icon: AnyView<State>,
    label: String,
}

/// A grid cell drawing `icon` above `label`.
///
/// The icon is a **view**, the child protocol [`crate::components::dock`]
/// established — the catalog ships no icon vocabulary.
pub fn bloom_menu_item<State: 'static, V: View<State>>(
    icon: V,
    label: impl Into<String>,
) -> BloomMenuItem<State> {
    BloomMenuItem {
        icon: any(icon),
        label: label.into(),
    }
}

// ---- The component ---------------------------------------------------------

/// A view-held selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A view-held open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// What the menu renders from, beyond its items.
#[derive(Clone, Debug, PartialEq)]
struct BloomConfig {
    open: bool,
    trigger_label: String,
    heading: String,
    close_label: String,
}

/// A declarative beUI bloom menu. See [`bloom_menu`].
pub struct BloomMenuView<State: 'static> {
    items: Vec<BloomMenuItem<State>>,
    config: BloomConfig,
    on_select: OnSelect<State>,
    on_open_change: OnOpenChange<State>,
}

/// Build a bloom menu over `items`: a labelled trigger that morphs open into a
/// three-column grid blooming out of its own centre.
///
/// The widget's own box is the centring box the trigger sits in the middle of
/// ([`BLOOM_BOX_HEIGHT`] tall), so opening the menu never reflows the page
/// around it — upstream's own arrangement, explained in the [module docs](self).
///
/// `on_select(state, index)` reports an activation with the item's index; the
/// menu also reports `false` through
/// [`on_open_change`](BloomMenuView::on_open_change) on the same event, which is
/// upstream's `onSelect?.(); setOpen(false)`.
pub fn bloom_menu<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<BloomMenuItem<State>>,
    on_select: F,
) -> BloomMenuView<State> {
    BloomMenuView {
        items,
        config: BloomConfig {
            open: false,
            trigger_label: "Create".to_string(),
            heading: "Create".to_string(),
            close_label: "Close menu".to_string(),
        },
        on_select: Rc::new(on_select),
        on_open_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> BloomMenuView<State> {
    /// Hand the menu the app's open flag. The default is `false`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.open = open;
        self
    }

    /// Set the closed trigger's label (upstream's `Create`).
    pub fn trigger_label(mut self, label: impl Into<String>) -> Self {
        self.config.trigger_label = label.into();
        self
    }

    /// Set the open panel's heading.
    pub fn heading(mut self, heading: impl Into<String>) -> Self {
        self.config.heading = heading.into();
        self
    }

    /// Set the close mark's accessible name (`aria-label="Close menu"`).
    pub fn close_label(mut self, label: impl Into<String>) -> Self {
        self.config.close_label = label.into();
        self
    }

    /// Set the open-change callback: the trigger reports `true`; the close mark,
    /// Escape, a press inside the box but outside the panel, and a selection all
    /// report `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// One laid-out cell.
struct Cell {
    label: LabelRun,
    /// The cell's box in the widget's own space.
    rect: Rect,
}

/// The retained widget for a [`BloomMenuView`].
pub struct BloomMenuWidget {
    /// One icon pod per cell.
    icons: Vec<ChildPod>,
    cells: Vec<Cell>,
    config: BloomConfig,
    trigger_label: LabelRun,
    heading: LabelRun,
    /// The morph between the trigger's box and the panel's, `0.0` closed.
    morph: Lane,
    /// The frame the current open episode started painting on.
    opened_at: Option<FrameTime>,
    /// The `reduce_motion` value the last paint resolved.
    reduced: bool,
    /// The trigger's box in the widget's own space.
    trigger: Rect,
    /// The panel's own box, once open.
    panel: Rect,
    /// The grid's box inside the panel.
    grid: Rect,
    /// The close mark's box.
    close: Rect,
    /// The cell the pointer or the keyboard is on.
    highlight: Option<usize>,
    /// The cell a primary `Down` armed.
    armed: Option<usize>,
    /// Whether the trigger (or the close mark) is holding a press.
    pressed: bool,
    on_select: ErasedArgCallback<usize>,
    on_open_change: ErasedArgCallback<bool>,
}

impl BloomMenuWidget {
    /// How far the morph has travelled: `0.0` on the trigger, `1.0` on the open
    /// panel.
    pub fn morph(&self) -> f64 {
        self.morph.value()
    }

    /// The trigger's box in the widget's own space.
    pub fn trigger_rect(&self) -> Rect {
        self.trigger
    }

    /// The open panel's box in the widget's own space.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    /// One cell's box in the widget's own space.
    pub fn cell_rect(&self, index: usize) -> Option<Rect> {
        self.cells.get(index).map(|cell| cell.rect)
    }

    /// The cell the highlight sits on.
    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// The morphing shell's box right now — the trigger's, the panel's, or the
    /// lerp between them.
    fn shell(&self) -> Rect {
        let t = self.morph.value().clamp(0.0, 1.0);
        Rect::new(
            lerp(self.trigger.x0, self.panel.x0, t),
            lerp(self.trigger.y0, self.panel.y0, t),
            lerp(self.trigger.x1, self.panel.x1, t),
            lerp(self.trigger.y1, self.panel.y1, t),
        )
    }

    /// The cell `position` lands on.
    fn cell_at(&self, position: Point) -> Option<usize> {
        self.cells
            .iter()
            .position(|cell| cell.rect.contains(position))
    }

    /// The cell `step` away in the grid, clamped at the edges: `±1` walks the
    /// row, `±BLOOM_COLUMNS` walks the column.
    fn step_highlight(&self, step: isize) -> Option<usize> {
        let count = self.cells.len();
        if count == 0 {
            return None;
        }
        let at = self.highlight.unwrap_or(0) as isize;
        Some((at + step).clamp(0, count as isize - 1) as usize)
    }

    /// Report a selection, then close — upstream's `onSelect?.(); setOpen(false)`.
    fn select(&mut self, ctx: &mut EventCtx, index: usize) {
        (self.on_select)(ctx, index);
        (self.on_open_change)(ctx, false);
    }
}

impl<State: 'static> View<State> for BloomMenuView<State> {
    type Element = BloomMenuWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BloomMenuWidget {
        BloomMenuWidget {
            icons: self
                .items
                .iter()
                .map(|item| build_child(&item.icon, ctx))
                .collect(),
            cells: self
                .items
                .iter()
                .map(|item| Cell {
                    label: LabelRun::new(item.label.clone()),
                    rect: Rect::ZERO,
                })
                .collect(),
            trigger_label: LabelRun::new(self.config.trigger_label.clone()),
            heading: LabelRun::new(self.config.heading.clone()),
            morph: Lane::at_rest(
                Ramp::spring(BLOOM_FOLDER),
                if self.config.open { 1.0 } else { 0.0 },
            ),
            config: self.config.clone(),
            opened_at: None,
            reduced: false,
            trigger: Rect::ZERO,
            panel: Rect::ZERO,
            grid: Rect::ZERO,
            close: Rect::ZERO,
            highlight: None,
            armed: None,
            pressed: false,
            on_select: erase_callback_arg(&self.on_select),
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BloomMenuWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.cells.len() != self.items.len() {
            element.cells = self
                .items
                .iter()
                .map(|item| Cell {
                    label: LabelRun::new(item.label.clone()),
                    rect: Rect::ZERO,
                })
                .collect();
            element.highlight = None;
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (cell, item) in element.cells.iter_mut().zip(&self.items) {
                if cell.label.set_content(item.label.clone()) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        if element.config != self.config {
            if element.config.open != self.config.open {
                // A fresh open episode: the iris, the fade and the radial delays
                // are all timed from the frame the panel next paints.
                element.opened_at = None;
                element.highlight = None;
                element.armed = None;
            }
            element
                .trigger_label
                .set_content(self.config.trigger_label.clone());
            element.heading.set_content(self.config.heading.clone());
            element.config = self.config.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.icons,
            ctx,
            |item: &BloomMenuItem<State>| &item.icon,
            |_| None::<ChildKey>,
        );
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_select = erase_callback_arg(&self.on_select);
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut BloomMenuWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.icons.iter_mut()) {
            teardown_child(&item.icon, pod, ctx);
        }
    }
}

/// A label's style (`text-sm font-medium`), in the theme's `label_large`
/// family.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK)
    };
    themed_style(style, ThemeTextType::LabelLarge, theme)
}

/// Paint lucide's `plus` mark — two strokes — centred on `centre`.
pub fn draw_plus(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let arm = extent / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, 0.0));
    path.line_to(Point::new(arm, 0.0));
    path.move_to(Point::new(0.0, -arm));
    path.line_to(Point::new(0.0, arm));
    scene.stroke_path(centre, &path, LUCIDE_STROKE, &Brush::Solid(color));
}

/// Paint lucide's `x` mark — two strokes — centred on `centre`.
pub fn draw_x(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let arm = extent / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, -arm));
    path.line_to(Point::new(arm, arm));
    path.move_to(Point::new(arm, -arm));
    path.line_to(Point::new(-arm, arm));
    scene.stroke_path(centre, &path, LUCIDE_STROKE, &Brush::Solid(color));
}

/// lucide's default `strokeWidth`, in logical px at this component's icon
/// sizes.
const LUCIDE_STROKE: f64 = 1.5;

impl Widget for BloomMenuWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let style = label_style(theme);
        self.trigger_label.layout(ctx, &style);
        self.heading.layout(ctx, &style);
        for cell in &mut self.cells {
            cell.label.layout(ctx, &style);
        }

        // `w-[min(86vw,420px)]`, against the width this mount actually offers.
        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            BLOOM_PANEL_MAX_WIDTH
        };
        let width = (available * BLOOM_PANEL_WIDTH_FRACTION).min(BLOOM_PANEL_MAX_WIDTH);
        let box_size = Size::new(width, BLOOM_BOX_HEIGHT);
        let centre = Point::new(box_size.width / 2.0, box_size.height / 2.0);

        self.trigger =
            Rect::from_center_size(centre, Size::new(BLOOM_TRIGGER_WIDTH, BLOOM_TRIGGER_HEIGHT));
        let rows = bloom_grid_rows(self.cells.len(), BLOOM_COLUMNS);
        let panel_height =
            BLOOM_HEADER_HEIGHT + style::BORDER_WIDTH + rows as f64 * BLOOM_CELL_HEIGHT;
        self.panel = Rect::from_center_size(centre, Size::new(width, panel_height));
        self.grid = Rect::new(
            self.panel.x0,
            self.panel.y0 + BLOOM_HEADER_HEIGHT + style::BORDER_WIDTH,
            self.panel.x1,
            self.panel.y1,
        );
        self.close = Rect::from_center_size(
            Point::new(
                self.panel.x1 - BLOOM_HEADER_PADDING_X - BLOOM_CLOSE_ICON / 2.0,
                self.panel.y0 + BLOOM_HEADER_HEIGHT / 2.0,
            ),
            Size::new(BLOOM_CLOSE_ICON, BLOOM_CLOSE_ICON),
        );

        let count = self.cells.len();
        let icon_bc = BoxConstraints::tight(Size::new(BLOOM_ICON_SIZE, BLOOM_ICON_SIZE));
        for index in 0..count {
            let rect = bloom_cell_rect(index, count, BLOOM_COLUMNS, self.grid);
            self.cells[index].rect = rect;
            if let Some(icon) = self.icons.get_mut(index) {
                icon.layout_child(ctx, &icon_bc);
                icon.set_origin(Point::new(
                    rect.center().x - BLOOM_ICON_SIZE / 2.0,
                    rect.y0 + BLOOM_CELL_PADDING_Y,
                ));
            }
        }
        bc.constrain(box_size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        self.reduced = reduce;
        let now = ctx.frame_time();
        let origin = ctx.origin();

        self.morph
            .retarget(if self.config.open { 1.0 } else { 0.0 });
        if reduce {
            self.morph.snap();
        }
        if self.morph.advance(now) {
            ctx.request_frame();
        }
        let progress = self.morph.value().clamp(0.0, 1.0);

        // The shell: one rounded rect travelling between the two boxes, both
        // centred on the same point.
        let shell = self.shell();
        paint_panel(scene, origin, shell, BLOOM_RADIUS, chrome);
        scene.push_clip_rounded(
            origin + shell.origin().to_vec2(),
            shell.size(),
            BLOOM_RADIUS,
        );

        if progress < 1.0 {
            // The trigger's own label and mark, fading out as the panel takes
            // over.
            let alpha = 1.0 - progress;
            let layered = alpha < 1.0;
            if layered {
                scene.push_layer(
                    origin + shell.origin().to_vec2(),
                    shell.size(),
                    alpha as f32,
                );
            }
            let text = self.trigger_label.size();
            let run = text.width + BLOOM_TRIGGER_GAP + BLOOM_PLUS_ICON;
            let left = shell.center().x - run / 2.0;
            self.trigger_label.paint(
                origin + Vec2::new(left, shell.center().y - text.height / 2.0),
                chrome.ink,
                scene,
            );
            draw_plus(
                scene,
                origin
                    + Vec2::new(
                        left + text.width + BLOOM_TRIGGER_GAP + BLOOM_PLUS_ICON / 2.0,
                        shell.center().y,
                    ),
                BLOOM_PLUS_ICON,
                chrome.ink,
            );
            if layered {
                scene.pop_layer();
            }
        }

        if self.config.open || progress > 0.0 {
            self.paint_panel_contents(ctx, scene, chrome, now, reduce);
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for icon in &mut self.icons {
                icon.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => self.handle_key(ctx, key),
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !self.config.open {
            let label = self.config.trigger_label.clone();
            ctx.push_node(Role::Button, |node| {
                node.set_label(label.as_str());
                node.set_expanded(false);
                node.add_action(Action::Click);
            });
            return;
        }
        let close_label = self.config.close_label.clone();
        ctx.push_container(
            Role::Menu,
            |_| {},
            |ctx| {
                for cell in &self.cells {
                    let label = cell.label.content().to_string();
                    ctx.push_node(Role::MenuItem, |node| {
                        node.set_label(label.as_str());
                        node.add_action(Action::Click);
                    });
                }
                ctx.push_node(Role::Button, |node| {
                    node.set_label(close_label.as_str());
                    node.add_action(Action::Click);
                });
            },
        );
    }

    visit_children!(icons);
}

impl BloomMenuWidget {
    /// Paint the header, the grid's hairlines and every cell, under the iris.
    fn paint_panel_contents(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        chrome: PanelChrome,
        now: FrameTime,
        reduce: bool,
    ) {
        let origin = ctx.origin();
        let opened = *self.opened_at.get_or_insert(now);
        let elapsed = now.saturating_sub(opened);

        // The contents' own delayed fade.
        let content = if reduce {
            1.0
        } else {
            elapsed
                .checked_sub(BLOOM_CONTENT_DELAY)
                .map_or(0.0, |since| {
                    Ramp::eased(BLOOM_CONTENT_FADE, EASE_OUT).progress_clamped(since)
                })
        };
        if content <= 0.0 {
            ctx.request_frame();
            return;
        }
        let layered = content < 1.0;
        if layered {
            scene.push_layer(
                origin + self.panel.origin().to_vec2(),
                self.panel.size(),
                content as f32,
            );
            ctx.request_frame();
        }

        // The header: heading, close mark, rule.
        let heading = self.heading.size();
        self.heading.paint(
            origin
                + Vec2::new(
                    self.panel.x0 + BLOOM_HEADER_PADDING_X,
                    self.panel.y0 + (BLOOM_HEADER_HEIGHT - heading.height) / 2.0,
                ),
            chrome.dim_ink,
            scene,
        );
        draw_x(
            scene,
            origin + self.close.center().to_vec2(),
            BLOOM_CLOSE_ICON,
            if self.pressed {
                chrome.ink
            } else {
                chrome.dim_ink
            },
        );
        scene.fill_rect(
            origin + Vec2::new(self.panel.x0, self.panel.y0 + BLOOM_HEADER_HEIGHT),
            Size::new(self.panel.width(), style::BORDER_WIDTH),
            chrome.border,
        );

        // The iris: the grid is revealed from a box at its own centre outward.
        let iris_progress = if reduce {
            1.0
        } else {
            elapsed.checked_sub(BLOOM_IRIS_DELAY).map_or(0.0, |since| {
                Ramp::eased(BLOOM_IRIS, EASE_OUT).progress_clamped(since)
            })
        };
        if iris_progress < 1.0 {
            ctx.request_frame();
        }
        let iris = bloom_iris_rect(self.grid, iris_progress);
        if iris.width() <= 0.0 || iris.height() <= 0.0 {
            if layered {
                scene.pop_layer();
            }
            return;
        }
        scene.push_clip(origin + iris.origin().to_vec2(), iris.size());

        // The cells' static hairlines, painted whole so the grid lines never
        // flicker as the items stagger in (upstream's own note).
        let count = self.cells.len();
        for index in 0..count {
            let rect = self.cells[index].rect;
            if index % BLOOM_COLUMNS != BLOOM_COLUMNS - 1 {
                scene.fill_rect(
                    origin + Vec2::new(rect.x1 - style::BORDER_WIDTH, rect.y0),
                    Size::new(style::BORDER_WIDTH, rect.height()),
                    chrome.border,
                );
            }
            if index / BLOOM_COLUMNS + 1 < bloom_grid_rows(count, BLOOM_COLUMNS) {
                scene.fill_rect(
                    origin + Vec2::new(rect.x0, rect.y1 - style::BORDER_WIDTH),
                    Size::new(rect.width(), style::BORDER_WIDTH),
                    chrome.border,
                );
            }
        }

        for index in 0..count {
            let reveal = bloom_item_reveal(elapsed, index, count, reduce);
            if reveal < 1.0 {
                ctx.request_frame();
            }
            if reveal <= 0.0 {
                continue;
            }
            self.paint_cell(ctx, scene, chrome, index, reveal);
        }

        scene.pop_clip();
        if layered {
            scene.pop_layer();
        }
    }

    /// Paint one cell's icon and label at its own point in the bloom.
    fn paint_cell(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        chrome: PanelChrome,
        index: usize,
        reveal: f64,
    ) {
        let origin = ctx.origin();
        let rect = self.cells[index].rect;
        let highlighted = self.highlight == Some(index);
        // `hover:text-foreground` over the resting `text-muted-foreground`.
        let ink = if highlighted {
            chrome.ink
        } else {
            chrome.dim_ink
        };
        let scale = BLOOM_ITEM_SCALE + (1.0 - BLOOM_ITEM_SCALE) * reveal;
        let centre = origin + rect.center().to_vec2();
        let layered = reveal < 1.0;
        if layered {
            scene.push_layer(origin + rect.origin().to_vec2(), rect.size(), reveal as f32);
        }
        scene.push_transform(
            Affine::translate(centre.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-centre.to_vec2()),
        );
        if let Some(icon) = self.icons.get_mut(index) {
            icon.paint_child(ctx, scene);
        }
        let cell = &self.cells[index];
        let text = cell.label.size();
        cell.label.paint(
            origin
                + Vec2::new(
                    rect.center().x - text.width / 2.0,
                    rect.y0 + BLOOM_CELL_PADDING_Y + BLOOM_ICON_SIZE + BLOOM_CELL_GAP,
                ),
            ink,
            scene,
        );
        scene.pop_transform();
        if layered {
            scene.pop_layer();
        }
    }

    /// The `Widget::event` key arm: the arrows walk the grid, Enter commits,
    /// Escape closes.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &frust::authoring::KeyEvent) -> EventResult {
        if !ctx.has_focus() {
            return EventResult::Ignored;
        }
        if key.key == Key::Named(NamedKey::Escape) {
            if !self.config.open {
                return EventResult::Ignored;
            }
            (self.on_open_change)(ctx, false);
            return EventResult::Handled;
        }
        if !self.config.open {
            // The closed trigger activates on Space or Enter like any button.
            if is_activation_key(key) {
                (self.on_open_change)(ctx, true);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let step = match &key.key {
            Key::Named(NamedKey::ArrowRight) => 1,
            Key::Named(NamedKey::ArrowLeft) => -1,
            Key::Named(NamedKey::ArrowDown) => BLOOM_COLUMNS as isize,
            Key::Named(NamedKey::ArrowUp) => -(BLOOM_COLUMNS as isize),
            _ => 0,
        };
        if step != 0 {
            let next = self.step_highlight(step);
            if self.highlight != next {
                self.highlight = next;
                ctx.request_redraw();
            }
            return EventResult::Handled;
        }
        if is_activation_key(key)
            && let Some(index) = self.highlight
        {
            self.select(ctx, index);
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    /// The `Widget::event` pointer arm: the trigger while shut, the grid and the
    /// close mark while open, and a light dismiss inside the centring box.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Ignored;
        }
        if !inside(p.position, ctx.size()) {
            return EventResult::Ignored;
        }
        if !self.config.open {
            return self.handle_trigger_pointer(ctx, p);
        }
        let on_panel = self.panel.contains(p.position);
        // The close mark's hit box is its own icon, grown to a comfortable
        // target — upstream's button is the icon plus its padding.
        let close = self
            .close
            .inflate(BLOOM_HEADER_PADDING_Y, BLOOM_HEADER_PADDING_Y);
        let on_close = close.contains(p.position);
        let cell = self.cell_at(p.position);
        match p.phase {
            PointerPhase::Move => {
                if on_panel {
                    ctx.claim_hover();
                }
                if on_close || cell.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.highlight != cell {
                    self.highlight = cell;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                ctx.capture_pointer();
                ctx.request_focus();
                if on_close {
                    self.pressed = true;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if let Some(index) = cell {
                    self.armed = Some(index);
                    self.highlight = Some(index);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if !on_panel {
                    // The light dismiss: a press inside the centring box but
                    // outside the panel. Latched here and acted on at `Up`, the
                    // press/release symmetry every other dismissal in this
                    // catalog keeps.
                    self.armed = None;
                    self.pressed = false;
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.pressed {
                    self.pressed = false;
                    ctx.request_redraw();
                    if on_close {
                        (self.on_open_change)(ctx, false);
                    }
                    return EventResult::Handled;
                }
                if let Some(armed) = self.armed.take() {
                    if cell == Some(armed) {
                        self.select(ctx, armed);
                    }
                    return EventResult::Handled;
                }
                if !on_panel {
                    (self.on_open_change)(ctx, false);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                let held = self.pressed || self.armed.is_some();
                self.pressed = false;
                self.armed = None;
                if held {
                    ctx.request_redraw();
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
        }
    }

    /// The pointer arm while the menu is shut: the trigger owns the whole
    /// gesture, and only a release inside it opens.
    fn handle_trigger_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let on_trigger = self.trigger.contains(p.position);
        match p.phase {
            PointerPhase::Move => {
                if on_trigger {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !on_trigger {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.pressed {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                ctx.request_redraw();
                if on_trigger {
                    (self.on_open_change)(ctx, true);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.pressed {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, escape, ft_ms, light, pointer, reduced};
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Key, KeyEvent, Modifiers, NamedKey};
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(600.0, 400.0);

    /// Long enough for the folder spring, the iris and every radial delay.
    const SETTLE_MS: f64 = 3_000.0;

    /// The six items upstream's own demo ships.
    const ITEMS: usize = 6;

    #[derive(Default)]
    struct App {
        open: bool,
        opens: Vec<bool>,
        selected: Vec<usize>,
    }

    fn icon<State: 'static>() -> impl View<State> {
        SizedBox(Some(BLOOM_ICON_SIZE), Some(BLOOM_ICON_SIZE))
    }

    fn items() -> Vec<BloomMenuItem<App>> {
        vec![
            bloom_menu_item(icon::<App>(), "Doc"),
            bloom_menu_item(icon::<App>(), "Board"),
            bloom_menu_item(icon::<App>(), "Table"),
            bloom_menu_item(icon::<App>(), "Folder"),
            bloom_menu_item(icon::<App>(), "Reminder"),
            bloom_menu_item(icon::<App>(), "Link"),
        ]
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::themed(light())
        }

        fn themed(theme: Theme) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                tcx: TextContext::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(theme));
            h.step(0.0);
            h
        }

        fn step(&mut self, ms: f64) {
            self.clock += ms;
            let mut logic = move |s: &mut App| {
                frust::Stack(vec![any(bloom_menu(items(), |s: &mut App, index| {
                    s.selected.push(index)
                })
                .open(s.open)
                .on_open_change(|s: &mut App, open| {
                    s.open = open;
                    s.opens.push(open);
                }))])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        fn read_after(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(self.clock));
            rec
        }

        fn read(&mut self) -> Recorder {
            self.read_after(SETTLE_MS)
        }

        fn settle(&mut self) {
            self.step(SETTLE_MS);
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// The morphing shell's box: the first rounded rect of the pass.
        fn shell(&mut self, rec: &Recorder) -> Rect {
            let _ = self;
            let (origin, size, _, _) = *rec.rrects.first().expect("the shell");
            Rect::from_origin_size(origin, size)
        }

        /// The settled shell.
        fn settled_shell(&mut self) -> Rect {
            let rec = self.read();
            self.shell(&rec)
        }

        /// Press the trigger and settle the bloom.
        fn open(&mut self) {
            let shell = self.settled_shell();
            let at = shell.center();
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.step(0.0);
            self.settle();
            self.settle();
        }

        fn key(&mut self, key: NamedKey) {
            self.event(InputEvent::Key(KeyEvent {
                key: Key::Named(key),
                modifiers: Modifiers::default(),
                repeat: false,
            }));
            self.settle();
        }

        /// The centre of cell `index`, in window space.
        fn cell_point(&mut self, index: usize) -> Point {
            let panel = self.settled_shell();
            let grid = Rect::new(
                panel.x0,
                panel.y0 + BLOOM_HEADER_HEIGHT + style::BORDER_WIDTH,
                panel.x1,
                panel.y1,
            );
            bloom_cell_rect(index, ITEMS, BLOOM_COLUMNS, grid).center()
        }
    }

    /// The panel's own height for the six-item demo grid.
    fn panel_height() -> f64 {
        BLOOM_HEADER_HEIGHT + style::BORDER_WIDTH + 2.0 * BLOOM_CELL_HEIGHT
    }

    // ---- The bloom arithmetic ---------------------------------------------

    #[test]
    fn the_grid_wraps_at_three_columns() {
        assert_eq!(bloom_grid_rows(6, 3), 2);
        assert_eq!(bloom_grid_rows(7, 3), 3, "a partial row still needs a row");
        assert_eq!(bloom_grid_rows(3, 3), 1);
        assert_eq!(bloom_grid_rows(0, 3), 0);
        assert_eq!(bloom_grid_rows(4, 0), 0, "no columns, no rows");
    }

    #[test]
    fn the_cells_tile_the_grid_row_major_and_leave_no_gutter() {
        let grid = Rect::new(0.0, 100.0, 400.0, 100.0 + 2.0 * BLOOM_CELL_HEIGHT);
        let cells: Vec<Rect> = (0..6)
            .map(|index| bloom_cell_rect(index, 6, BLOOM_COLUMNS, grid))
            .collect();

        // Row-major: the first three share a row, the next three the one below.
        assert_eq!(cells[0].y0, grid.y0);
        assert_eq!(cells[2].y0, grid.y0);
        assert_eq!(cells[3].y0, grid.y0 + BLOOM_CELL_HEIGHT);
        assert_eq!(cells[0].height(), BLOOM_CELL_HEIGHT);

        // Edge to edge, with no seam between neighbours and none at the ends.
        assert_eq!(cells[0].x0, grid.x0);
        assert_eq!(cells[0].x1, cells[1].x0);
        assert_eq!(cells[1].x1, cells[2].x0);
        assert_eq!(
            cells[2].x1, grid.x1,
            "the trailing column takes the remainder"
        );
        assert_eq!(cells[5].y1, grid.y1);

        // Out of range is empty rather than wrapping onto a phantom row.
        assert_eq!(
            bloom_cell_rect(6, 6, BLOOM_COLUMNS, grid).size(),
            Size::ZERO
        );
        assert_eq!(bloom_cell_rect(0, 6, 0, grid).size(), Size::ZERO);
    }

    #[test]
    fn a_ragged_last_row_keeps_its_cells_on_the_leading_columns() {
        let grid = Rect::new(0.0, 0.0, 300.0, 2.0 * BLOOM_CELL_HEIGHT);
        // Four items: three on the first row, one alone on the second.
        let fourth = bloom_cell_rect(3, 4, BLOOM_COLUMNS, grid);
        assert_eq!(
            fourth.x0, grid.x0,
            "it starts a new row at the leading edge"
        );
        assert_eq!(fourth.y0, grid.y0 + BLOOM_CELL_HEIGHT);
        assert_eq!(fourth.width(), 100.0);
    }

    #[test]
    fn the_radial_distance_puts_every_corner_at_the_same_remove() {
        // Six items over three columns: the centre is (1, 0.5).
        let corners: Vec<f64> = [0usize, 2, 3, 5]
            .iter()
            .map(|index| bloom_radial_distance(*index, 6, BLOOM_COLUMNS))
            .collect();
        let first = corners[0];
        assert!(
            corners.iter().all(|d| (d - first).abs() < 1e-12),
            "the four corners are equidistant, which is what makes the open read \
             centre-out: {corners:?}"
        );
        assert!((first - (1.0f64).hypot(0.5)).abs() < 1e-12);

        // The middle column is nearer than the corners.
        for index in [1usize, 4] {
            let middle = bloom_radial_distance(index, 6, BLOOM_COLUMNS);
            assert!((middle - 0.5).abs() < 1e-12, "{middle}");
            assert!(middle < first);
        }
        assert_eq!(
            bloom_radial_distance(9, 6, BLOOM_COLUMNS),
            0.0,
            "out of range"
        );
    }

    #[test]
    fn the_item_delay_is_the_base_plus_its_own_distance() {
        let centre = bloom_item_delay(1, 6, BLOOM_COLUMNS);
        let corner = bloom_item_delay(0, 6, BLOOM_COLUMNS);
        assert_eq!(
            centre,
            BLOOM_ITEM_BASE_DELAY + BLOOM_ITEM_DELAY_PER_UNIT.mul_f64(0.5)
        );
        assert!(corner > centre, "the corners wait for the middle");
        assert_eq!(
            bloom_item_delay(2, 6, BLOOM_COLUMNS),
            corner,
            "and arrive together"
        );
        // The column count is fixed at three whatever the item count is
        // (upstream's `const cols = 3`), so a lone item sits in the *leading*
        // column, one full column from the grid's centre — not on it.
        assert_eq!(bloom_radial_distance(0, 1, BLOOM_COLUMNS), 1.0);
        assert_eq!(
            bloom_item_delay(0, 1, BLOOM_COLUMNS),
            BLOOM_ITEM_BASE_DELAY + BLOOM_ITEM_DELAY_PER_UNIT
        );
    }

    #[test]
    fn the_iris_opens_from_the_grids_own_centre() {
        let grid = Rect::new(0.0, 0.0, 400.0, 200.0);
        let folded = bloom_iris_rect(grid, 0.0);
        assert_eq!(folded.x0, 400.0 * BLOOM_IRIS_INSET_X);
        assert_eq!(folded.y0, 200.0 * BLOOM_IRIS_INSET_Y);
        assert_eq!(folded.center(), grid.center(), "centred on the grid");
        assert!(folded.width() > 0.0 && folded.height() > 0.0);

        assert_eq!(bloom_iris_rect(grid, 1.0), grid);
        assert_eq!(
            bloom_iris_rect(grid, 2.0),
            grid,
            "an overshoot cannot tear it"
        );
        assert_eq!(bloom_iris_rect(grid, -1.0), folded);

        let half = bloom_iris_rect(grid, 0.5);
        assert!(half.width() > folded.width() && half.width() < grid.width());
    }

    #[test]
    fn an_item_is_absent_until_its_own_delay_and_reduce_motion_skips_every_delay() {
        let delay = bloom_item_delay(0, 6, BLOOM_COLUMNS);
        assert_eq!(bloom_item_reveal(Duration::ZERO, 0, 6, false), 0.0);
        assert_eq!(bloom_item_reveal(delay, 0, 6, false), 0.0);
        assert!(bloom_item_reveal(delay + Duration::from_millis(40), 0, 6, false) > 0.0);
        assert_eq!(bloom_item_reveal(Duration::from_secs(5), 0, 6, false), 1.0);
        assert_eq!(bloom_item_reveal(Duration::ZERO, 0, 6, true), 1.0);
    }

    // ---- The widget -------------------------------------------------------

    #[test]
    fn the_closed_menu_is_the_trigger_centred_in_a_fixed_box() {
        let mut h = Harness::new();
        let shell = h.settled_shell();
        assert_eq!(
            shell.size(),
            Size::new(BLOOM_TRIGGER_WIDTH, BLOOM_TRIGGER_HEIGHT)
        );
        // The box is `min(86vw, 420)` wide and 300 tall whatever the menu is
        // doing, so the page around it never reflows.
        let expected = (WINDOW.width * BLOOM_PANEL_WIDTH_FRACTION).min(BLOOM_PANEL_MAX_WIDTH);
        assert_eq!(
            shell.center(),
            Point::new(expected / 2.0, BLOOM_BOX_HEIGHT / 2.0)
        );
    }

    #[test]
    fn the_trigger_opens_on_a_release_inside_it_and_not_on_the_press() {
        let mut h = Harness::new();
        let at = h.settled_shell().center();
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        assert!(h.state.opens.is_empty(), "up-inside, not down");
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        assert_eq!(h.state.opens, vec![true]);
    }

    #[test]
    fn the_panel_grows_out_of_the_trigger_about_the_same_centre() {
        let mut h = Harness::new();
        let closed = h.settled_shell();
        h.open();
        let open = h.settled_shell();
        assert_eq!(
            open.width(),
            (WINDOW.width * BLOOM_PANEL_WIDTH_FRACTION).min(BLOOM_PANEL_MAX_WIDTH)
        );
        assert_eq!(open.height(), panel_height());
        assert!(
            (open.center() - closed.center()).hypot() < 0.001,
            "both boxes share their centre, which is what makes it bloom"
        );
    }

    #[test]
    fn the_morph_passes_through_the_boxes_between_the_two() {
        let mut h = Harness::new();
        let at = h.settled_shell().center();
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        h.step(0.0);
        let first = h.read_after(0.0);
        let shell = h.shell(&first);
        assert_eq!(
            shell.size(),
            Size::new(BLOOM_TRIGGER_WIDTH, BLOOM_TRIGGER_HEIGHT),
            "the run starts on the trigger"
        );
        let mid = h.read_after(60.0);
        let shell = h.shell(&mid);
        assert!(
            shell.height() > BLOOM_TRIGGER_HEIGHT && shell.height() < panel_height(),
            "mid-morph: {shell:?}"
        );
    }

    #[test]
    fn reduce_motion_opens_on_the_spot() {
        let mut h = Harness::themed(reduced());
        let at = h.settled_shell().center();
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        h.step(0.0);
        let first = h.read_after(0.0);
        let shell = h.shell(&first);
        assert_eq!(shell.height(), panel_height(), "no morph at all");
        // ...and every cell is already there, with no radial delay left.
        assert_eq!(bloom_item_reveal(Duration::ZERO, 5, ITEMS, true), 1.0);
    }

    // ---- Selection and dismissal ------------------------------------------

    #[test]
    fn a_click_on_a_cell_selects_it_and_closes_the_menu() {
        let mut h = Harness::new();
        h.open();
        let at = h.cell_point(4);
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        assert_eq!(h.state.selected, vec![4]);
        assert_eq!(h.state.opens, vec![true, false]);
    }

    #[test]
    fn a_press_that_releases_off_its_cell_selects_nothing() {
        let mut h = Harness::new();
        h.open();
        let first = h.cell_point(0);
        let other = h.cell_point(1);
        h.event(pointer(PointerPhase::Down, first.x, first.y));
        h.event(pointer(PointerPhase::Up, other.x, other.y));
        assert!(h.state.selected.is_empty());
        assert!(h.state.open, "and the menu is still open");
    }

    #[test]
    fn escape_closes_the_open_menu() {
        let mut h = Harness::new();
        h.open();
        h.event(escape());
        assert_eq!(h.state.opens, vec![true, false]);
    }

    #[test]
    fn a_press_inside_the_box_but_outside_the_panel_dismisses() {
        let mut h = Harness::new();
        h.open();
        let panel = h.settled_shell();
        // The centring box is taller than the panel, so its top strip is inside
        // the widget and outside the menu.
        let at = Point::new(panel.center().x, panel.y0 / 2.0);
        assert!(at.y < panel.y0, "the strip really is above the panel");
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        assert_eq!(h.state.opens, vec![true, false]);
    }

    #[test]
    fn the_close_mark_shuts_the_menu() {
        let mut h = Harness::new();
        h.open();
        let panel = h.settled_shell();
        let at = Point::new(
            panel.x1 - BLOOM_HEADER_PADDING_X - BLOOM_CLOSE_ICON / 2.0,
            panel.y0 + BLOOM_HEADER_HEIGHT / 2.0,
        );
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        assert_eq!(h.state.opens, vec![true, false]);
        assert!(h.state.selected.is_empty(), "the header is not a cell");
    }

    #[test]
    fn the_arrows_walk_the_grid_and_enter_commits() {
        let mut h = Harness::new();
        h.open();
        // A press on the panel claims the focus the keys route by; pressing a
        // cell and releasing off it arms nothing.
        let cell = h.cell_point(0);
        let elsewhere = h.cell_point(1);
        h.event(pointer(PointerPhase::Down, cell.x, cell.y));
        h.event(pointer(PointerPhase::Up, elsewhere.x, elsewhere.y));
        h.settle();

        // Right walks the row; Down walks the column.
        h.key(NamedKey::ArrowRight);
        h.key(NamedKey::ArrowDown);
        h.event(InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        }));
        assert_eq!(
            h.state.selected,
            vec![1 + BLOOM_COLUMNS],
            "one right and one down from the first cell"
        );
    }

    #[test]
    fn the_keyboard_walk_clamps_at_the_grids_edges() {
        let mut h = Harness::new();
        h.open();
        let cell = h.cell_point(0);
        h.event(pointer(PointerPhase::Down, cell.x, cell.y));
        h.event(pointer(PointerPhase::Up, cell.x, cell.y));
        // That press selected and closed it; reopen and walk from the top-left.
        h.step(0.0);
        h.state.open = true;
        h.state.selected.clear();
        h.settle();
        let cell = h.cell_point(0);
        h.event(pointer(PointerPhase::Down, cell.x, cell.y));
        h.event(pointer(PointerPhase::Up, cell.x, cell.y));
        h.state.open = true;
        h.state.selected.clear();
        h.settle();

        for _ in 0..10 {
            h.key(NamedKey::ArrowUp);
        }
        h.event(InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        }));
        assert_eq!(h.state.selected, vec![0], "clamped at the first cell");
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn the_open_panel_paints_its_header_its_hairlines_and_every_cell() {
        let mut h = Harness::new();
        h.open();
        let rec = h.read();
        assert!(!rec.shadows.is_empty(), "the panel casts the glass shadow");
        assert!(rec.strokes > 0, "the hairline and the close mark");
        assert!(!rec.clips.is_empty(), "the shell clip and the iris");
        // Six cell labels plus the heading.
        assert_eq!(rec.inks.len(), ITEMS + 1);
        // The header rule plus the grid's own hairlines: two verticals per row
        // and one horizontal per cell of the first row.
        assert!(
            rec.rects.len() >= 1 + 4 + 3,
            "the static grid lines are painted whole: {}",
            rec.rects.len()
        );
    }

    #[test]
    fn the_closed_menu_paints_its_trigger_label_and_plus_mark() {
        let mut h = Harness::new();
        let rec = h.read();
        assert_eq!(rec.inks.len(), 1, "just the trigger's label");
        assert!(rec.strokes > 0, "the hairline and the plus");
    }

    // ---- Typeface: the trigger, heading and item labels follow the theme ----

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// The menu with glyph-free icons, closed (the trigger's label) or open
    /// (the heading and every item's label).
    fn probe_logic(open: bool) -> impl FnMut(&mut ()) -> frust::StackView<()> {
        move |_: &mut ()| {
            let items = ["Doc", "Board", "Table"]
                .into_iter()
                .map(|label| bloom_menu_item(icon::<()>(), label))
                .collect();
            frust::Stack(vec![any(
                bloom_menu::<(), _>(items, |_: &mut (), _| {}).open(open)
            )])
        }
    }

    #[test]
    fn menu_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the closed menu's trigger", probe_logic(false), WINDOW);
        assert_paints_only_in_geist("the open menu's labels", probe_logic(true), WINDOW);
        let open = Probe::new(probe_logic(true), WINDOW, crate::theme()).frame();
        assert_eq!(open.len(), 4, "the heading and three item labels");
    }

    #[test]
    fn menu_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the closed menu's trigger", probe_logic(false), WINDOW);
        assert_follows_a_live_family_swap("the open menu's labels", probe_logic(true), WINDOW);
    }
}
