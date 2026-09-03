//! Ports beUI's `context-menu` component — `components/motion/context-menu.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `VIEWPORT_PADDING = 8` | [`crate::overlay::VIEWPORT_PADDING`] |
//! | `MORPH_DURATION = 0.3` | [`CONTEXT_MENU_MORPH`] |
//! | `collapsedClip`'s `half = 8` | [`CONTEXT_MENU_SEED`] |
//! | panel `min-w-56 rounded-xl border border-border bg-card p-1.5` | [`CONTEXT_MENU_MIN_WIDTH`], [`style::RADIUS_XL`], [`CONTEXT_MENU_PADDING`] |
//! | collapsed `round 10px` → open `round 12px` | [`CONTEXT_MENU_SEED_RADIUS`] → [`style::RADIUS_XL`] |
//! | item `gap-2.5 rounded-lg px-2.5 py-2 text-[13px]` | [`CONTEXT_MENU_ITEM_*`](CONTEXT_MENU_ITEM_HEIGHT) |
//! | active pill `bg-foreground/[0.065]` on `SPRING_LAYOUT` | [`CONTEXT_MENU_PILL_ALPHA`] |
//! | destructive `text-destructive`, pill `bg-destructive/10` | [`ContextMenuTone::Destructive`] |
//! | `disabled:opacity-40` | [`CONTEXT_MENU_DISABLED_OPACITY`] |
//! | label `px-2.5 pt-1.5 pb-1 text-[10px] font-semibold uppercase` | [`context_menu_label`] |
//! | separator `-mx-1 my-1 h-px bg-border` | [`context_menu_separator`] |
//! | shortcut `ml-auto pl-4 text-[10px] font-medium text-muted-foreground` | [`ContextMenuItem::shortcut`] |
//!
//! # The panel unfolds from the press point
//!
//! `collapsedClip` builds an `inset(...)` that leaves a 16×16 square centred on
//! the pointer showing, and animates it out to the whole panel over
//! [`CONTEXT_MENU_MORPH`]. That is the same "rounded rect travelling from a
//! source rect to the panel's own" mechanism
//! [`crate::components::popover`](crate::components::popover::morph_rect)
//! already carries, so the popover's lerp is reused rather than re-derived; only
//! the source rect differs (a seed square at the pointer, rather than the
//! trigger's box or a corner sliver).
//!
//! # Mounting
//!
//! The panel mounts through [`crate::overlay::anchored`]: no component here
//! builds a host. [`context_menu_trigger`] publishes the **press point** as a
//! zero-size anchor rect, and the placement is `bottom`/`start` at zero offset,
//! so the panel's leading top corner lands on the pointer and
//! [`crate::overlay::place`]'s clamp keeps it inside the window — which is
//! exactly what `context-menu.tsx`'s own `Math.max(VIEWPORT_PADDING, …)`
//! positioning does, and the only collision handling upstream ships. The flip is
//! off for the same reason: upstream does not flip, it clamps.
//!
//! # Secondary-button reach
//!
//! `PointerEvent::button` carries
//! [`PointerButton::Secondary`](frust::authoring::PointerButton::Secondary),
//! which is what the trigger matches on: `frust-shell-desktop` forwards the
//! right mouse button as a secondary press, so a right-click over the trigger
//! area opens the menu at the pointer. The same routing
//! `plugins/shadcn/src/components/context_menu.rs` established, and the reason
//! the trigger claims that button *before* routing to its child — no baseline
//! control in any catalog does anything with a secondary press, while the whole
//! primary gesture stays the child's.
//!
//! # Degradations against the web original
//!
//! - **No submenus.** Upstream ships none either (this is a premise correction:
//!   `context-menu.tsx` has a `ContextMenuItem`, checkbox/radio items, a label
//!   and a separator, and no `Sub`/`SubTrigger` pair at all), so there is
//!   nothing to port.
//! - **No checkbox or radio items.** `ContextMenuCheckboxItem` and
//!   `ContextMenuRadioGroup`/`RadioItem` carry a lucide `Check` and a dot, and a
//!   group-selection model the app would have to own. The catalog has no icon
//!   vocabulary to draw the mark from, so they are left out rather than
//!   half-drawn; a caller composes the same effect with a
//!   [`ContextMenuItem::shortcut`] mark today.
//! - **No typeahead, and no long-press.** Upstream's `typeahead` buffer and its
//!   `LONG_PRESS_DELAY` touch path both need machinery a widget here has no
//!   route to (a text buffer with its own timeout; a pointer *type* that
//!   `PointerEvent` does not carry). Keyboard **arrow** navigation is ported —
//!   see below.
//! - **Arrow navigation moves a menu-local active index, not focus.** Upstream
//!   moves DOM focus between item buttons; the whole menu is one widget here, so
//!   there is no per-item focus target. The visible effect — the pill following
//!   the active row on `SPRING_LAYOUT` — is the same.
//! - **The keyboard is reachable only once the menu holds focus.** Upstream
//!   focuses the first enabled item on the frame the menu opens; frust has no
//!   auto-focus-on-appear hook, which [`crate::overlay::anchored`] records as its
//!   own limitation, and the trigger still holds the focus its secondary press
//!   claimed. The host claims focus on the first press that lands *on the panel*,
//!   and the arrow keys reach the rows from there. Nothing else about the
//!   navigation differs.
//! - **The staggered item entrance is an addition.** Upstream fades the whole
//!   panel's contents with the clip, uniformly. The porting card asks for a
//!   stagger, so the rows arrive [`CONTEXT_MENU_STAGGER`] apart on top of the
//!   clip; `reduce_motion` collapses it back to upstream's single beat.
//! - **The drop shadow is the `.glass` chrome recipe** every panel in this
//!   catalog casts, not upstream's own
//!   `drop-shadow(0 18px 28px rgba(0,0,0,0.2))` — one shadow recipe per catalog
//!   is what the overlay hosts reserve fade-layer room for.
//! - **Controlled only**, like every other widget here: the app owns the open
//!   flag and the trigger reports the request.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerButton,
    PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{PanelChrome, lerp, morph_rect, paint_panel, resolve_panel};
use crate::motion::{Presence, PresencePhase, Ramp, Stagger};
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};

/// `min-w-56` — the panel's minimum width, in logical px.
pub const CONTEXT_MENU_MIN_WIDTH: f64 = 224.0;

/// `p-1.5` — the panel's own padding around its rows, in logical px.
pub const CONTEXT_MENU_PADDING: f64 = 6.0;

/// `py-2` plus a `text-[13px]` line — one item row's height, in logical px.
pub const CONTEXT_MENU_ITEM_HEIGHT: f64 = 34.0;

/// `px-2.5` — an item row's horizontal padding, in logical px.
pub const CONTEXT_MENU_ITEM_PADDING_X: f64 = 10.0;

/// `pl-4` — the least gap between a label and its shortcut, in logical px.
pub const CONTEXT_MENU_SHORTCUT_GAP: f64 = 16.0;

/// `text-[13px]` — an item label's size, in logical px.
pub const CONTEXT_MENU_TEXT: f64 = 13.0;

/// `text-[10px]` — the size of a section label and of a shortcut, in logical px.
pub const CONTEXT_MENU_SMALL_TEXT: f64 = 10.0;

/// `my-1` — the vertical margin around a separator, in logical px.
pub const CONTEXT_MENU_SEPARATOR_MARGIN: f64 = 4.0;

/// `-mx-1` — how far a separator bleeds past the panel padding on each side, in
/// logical px.
pub const CONTEXT_MENU_SEPARATOR_BLEED: f64 = 4.0;

/// A section label's row height (`pt-1.5 pb-1` around a `text-[10px]` line), in
/// logical px.
pub const CONTEXT_MENU_LABEL_HEIGHT: f64 = 22.0;

/// `bg-foreground/[0.065]` — the active pill's alpha over the ink role.
pub const CONTEXT_MENU_PILL_ALPHA: f32 = 0.065;

/// `bg-destructive/10` — the destructive active pill's alpha.
pub const CONTEXT_MENU_DANGER_PILL_ALPHA: f32 = 0.10;

/// `disabled:opacity-40` — a disabled row's ink opacity.
pub const CONTEXT_MENU_DISABLED_OPACITY: f32 = 0.40;

/// `MORPH_DURATION` — how long the panel takes to unfold from the press point,
/// and to fold back.
pub const CONTEXT_MENU_MORPH: Duration = Duration::from_millis(300);

/// `collapsedClip`'s `half = 8`: the seed square left showing at the pointer is
/// twice this on each axis.
pub const CONTEXT_MENU_SEED: f64 = 8.0;

/// The seed square's corner radius (`collapsedClip`'s own `round 10px`), in
/// logical px.
pub const CONTEXT_MENU_SEED_RADIUS: f64 = 10.0;

/// How far apart the rows arrive during the entrance. An addition — see the
/// [module docs](self).
pub const CONTEXT_MENU_STAGGER: Duration = Duration::from_millis(18);

/// How long one row's own entrance takes.
pub const CONTEXT_MENU_ROW_ENTER: Duration = Duration::from_millis(160);

/// How far below its resting position a row starts, in logical px.
pub const CONTEXT_MENU_ROW_LIFT: f64 = 6.0;

/// A row's tone — `context-menu.tsx`'s `ContextMenuItemTone`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextMenuTone {
    /// `text-foreground`, pill `bg-foreground/[0.065]`.
    #[default]
    Default,
    /// `text-destructive`, pill `bg-destructive/10`.
    Destructive,
}

/// What a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    /// A selectable `ContextMenuItem`.
    Item,
    /// A `ContextMenuSeparator` rule.
    Separator,
    /// A `ContextMenuLabel` section heading.
    Label,
}

/// One row of a context menu: an item, a separator, or a section label.
///
/// Build one with [`context_menu_item`], [`context_menu_separator`] or
/// [`context_menu_label`].
#[derive(Clone, Debug)]
pub struct ContextMenuItem {
    kind: RowKind,
    label: String,
    shortcut: Option<String>,
    disabled: bool,
    tone: ContextMenuTone,
}

/// A selectable menu item showing `label`.
pub fn context_menu_item(label: impl Into<String>) -> ContextMenuItem {
    ContextMenuItem {
        kind: RowKind::Item,
        label: label.into(),
        shortcut: None,
        disabled: false,
        tone: ContextMenuTone::default(),
    }
}

/// A horizontal rule between groups of items.
pub fn context_menu_separator() -> ContextMenuItem {
    ContextMenuItem {
        kind: RowKind::Separator,
        label: String::new(),
        shortcut: None,
        disabled: true,
        tone: ContextMenuTone::default(),
    }
}

/// A section heading — upstream's `ContextMenuLabel`, uppercased by the caller
/// (the catalog shapes text, it does not transform it).
pub fn context_menu_label(label: impl Into<String>) -> ContextMenuItem {
    ContextMenuItem {
        kind: RowKind::Label,
        label: label.into(),
        shortcut: None,
        disabled: true,
        tone: ContextMenuTone::default(),
    }
}

impl ContextMenuItem {
    /// Set the trailing shortcut hint (`ContextMenuShortcut`).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Make this item unselectable (`disabled:opacity-40`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the row's tone.
    pub fn tone(mut self, tone: ContextMenuTone) -> Self {
        self.tone = tone;
        self
    }

    /// Whether this row can be activated.
    fn selectable(&self) -> bool {
        self.kind == RowKind::Item && !self.disabled
    }

    /// This row's own height, in logical px.
    fn height(&self) -> f64 {
        match self.kind {
            RowKind::Item => CONTEXT_MENU_ITEM_HEIGHT,
            RowKind::Label => CONTEXT_MENU_LABEL_HEIGHT,
            RowKind::Separator => style::BORDER_WIDTH + CONTEXT_MENU_SEPARATOR_MARGIN * 2.0,
        }
    }
}

/// An item's label style (`text-[13px]`).
fn item_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        ..TextStyle::new(CONTEXT_MENU_TEXT as f32, crate::text::SHAPING_INK)
    }
}

/// A shortcut's style (`text-[10px] font-medium`).
fn shortcut_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_small.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(CONTEXT_MENU_SMALL_TEXT as f32, crate::text::SHAPING_INK)
    }
}

/// A section label's style (`text-[10px] font-semibold`).
fn section_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..shortcut_style(theme)
    }
}

// ---- The component ---------------------------------------------------------

/// The mutable configuration the outer builder writes and the inner panel reads
/// — the handle shape [`crate::components::popover`] documents.
type MenuHandle = Rc<RefCell<MenuConfig>>;

/// A view-held selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A view-held open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// What the panel needs from its component's builders.
#[derive(Clone, Debug)]
struct MenuConfig {
    open: bool,
    anchor: OverlayAnchor,
}

/// Equality over what the panel renders from; the shared [`OverlayAnchor`] cell
/// carries no identity comparison and is re-seated unconditionally instead.
impl PartialEq for MenuConfig {
    fn eq(&self, other: &Self) -> bool {
        self.open == other.open
    }
}

/// A declarative beUI context menu. See [`context_menu`].
pub struct ContextMenuView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    config: MenuHandle,
    placement: OverlayPlacement,
}

/// Build a context-menu panel over `items`, to be mounted as the top child of a
/// full-area [`frust::Stack`] and handed the app's own open flag through
/// [`ContextMenuView::open`].
///
/// `on_select(state, index)` reports an activation with the item's index in the
/// `items` vector — separators and labels included, so the index a caller reads
/// is the index it wrote.
pub fn context_menu<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<ContextMenuItem>,
    on_select: F,
) -> ContextMenuView<State> {
    let config: MenuHandle = Rc::new(RefCell::new(MenuConfig {
        open: true,
        anchor: OverlayAnchor::new(),
    }));
    // Anchored to a *point*: the panel's leading top corner lands on it, and the
    // host's clamp keeps the panel inside the window.
    let placement = OverlayPlacement::on(OverlaySide::Bottom)
        .align(OverlayAlign::Start)
        .offset(0.0)
        .flip(false);
    let panel = ContextMenuPanelView {
        items,
        config: config.clone(),
        on_select: Rc::new(on_select),
    };
    ContextMenuView {
        inner: anchored(panel)
            .placement(placement)
            .exit(Ramp::eased(CONTEXT_MENU_MORPH, EASE_OUT)),
        config,
        placement,
    }
}

impl<State: 'static> ContextMenuView<State> {
    /// Anchor the menu to the point (or box) `anchor` carries — the cell
    /// [`context_menu_trigger`] writes the press point into.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.config.borrow_mut().anchor = anchor.clone();
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the panel opens on (default [`OverlaySide::Bottom`], from
    /// the press point).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Hand the panel the app's open flag. The default is `true`.
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

    /// Set the exit-finished callback — the host has settled closed.
    pub fn on_exited<F: Fn(&mut State) + 'static>(mut self, on_exited: F) -> Self {
        self.inner = self.inner.on_exited(on_exited);
        self
    }
}

impl<State: 'static> View<State> for ContextMenuView<State> {
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

/// The menu's panel: the surface, the rows, the pill and the unfold.
struct ContextMenuPanelView<State: 'static> {
    items: Vec<ContextMenuItem>,
    config: MenuHandle,
    on_select: OnSelect<State>,
}

/// One laid-out row.
struct Row {
    item: ContextMenuItem,
    label: LabelRun,
    shortcut: Option<LabelRun>,
    /// The row's box in the panel's own space.
    rect: Rect,
}

/// The retained widget for a context-menu panel.
pub struct ContextMenuPanelWidget {
    rows: Vec<Row>,
    config: MenuConfig,
    /// Drives the unfold, opened and closed with the host.
    morph: Presence,
    /// The `reduce_motion` value the drivers were last built for.
    reduced: Option<bool>,
    /// The row the pill sits on — hover or keyboard.
    active: Option<usize>,
    /// The row a primary `Down` armed, with its snapshot to match identity at `Up`.
    /// Stored as (index, item_snapshot) so a same-length swap between Down and Up
    /// requires both geometric re-hit and identity match.
    armed: Option<(usize, ContextMenuItem)>,
    /// The pill's travel between rows, `0.0` at `pill_from`, `1.0` at the active
    /// row (`SPRING_LAYOUT`, upstream's own shared-layout spring).
    pill: Lane,
    pill_from: Rect,
    pill_to: Rect,
    /// The pill's opacity — `1.0` while a row is active, springing to `0.0`
    /// when none is. Kept apart from `pill` so a row-to-row move restarts the
    /// travel and never the alpha: the pill slides at its resting alpha and
    /// only fades when it is shown from, or sent to, hidden.
    pill_alpha: Lane,
    /// When the current entrance started, for the row stagger.
    entered_at: Option<FrameTime>,
    on_select: ErasedArgCallback<usize>,
}

impl ContextMenuPanelWidget {
    /// The row the pill currently sits on.
    pub fn active_row(&self) -> Option<usize> {
        self.active
    }

    /// The unfold's progress at the last paint.
    pub fn morph_phase(&self) -> PresencePhase {
        self.morph.phase()
    }

    /// One row's box in the panel's own space.
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        self.rows.get(index).map(|row| row.rect)
    }

    /// Move the pill onto `next`, springing from wherever it is now.
    fn set_active(&mut self, next: Option<usize>) -> bool {
        if self.active == next {
            return false;
        }
        let current = self.pill_rect();
        self.active = next;
        if let Some(index) = next.and_then(|i| self.rows.get(i)) {
            // A pill arriving from nowhere starts on its own row rather than
            // sliding in from the panel origin.
            self.pill_from = if self.pill_alpha.target() == 0.0 && current.is_zero_area() {
                index.rect
            } else {
                current
            };
            self.pill_to = index.rect;
            // New endpoints: the travel restarts at 0 — `pill_from` holds the
            // displayed rect, so a mid-flight pill continues from where it
            // is — and springs to 1. The alpha lane is not touched by a
            // row-to-row move; it only climbs when the pill was hidden.
            self.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0);
            self.pill.retarget(1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(SPRING_LAYOUT), 1.0);
        } else {
            // Hide: the rect stays put and only the alpha fades out.
            self.pill_from = current;
            self.pill_to = current;
            self.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(SPRING_LAYOUT), 0.0);
        }
        true
    }

    /// The pill's rect right now.
    fn pill_rect(&self) -> Rect {
        let t = self.pill.value().clamp(0.0, 1.0);
        Rect::new(
            lerp(self.pill_from.x0, self.pill_to.x0, t),
            lerp(self.pill_from.y0, self.pill_to.y0, t),
            lerp(self.pill_from.x1, self.pill_to.x1, t),
            lerp(self.pill_from.y1, self.pill_to.y1, t),
        )
    }

    /// The next selectable row in `step`'s direction, wrapping.
    fn step_active(&self, step: isize) -> Option<usize> {
        let count = self.rows.len();
        if count == 0 {
            return None;
        }
        let start = self
            .active
            .map_or(if step > 0 { count - 1 } else { 0 }, |i| i);
        for hop in 1..=count {
            let offset = step * hop as isize;
            let index = (start as isize + offset).rem_euclid(count as isize) as usize;
            if self.rows[index].item.selectable() {
                return Some(index);
            }
        }
        None
    }

    /// The row `position` (in the panel's own space) lands on, if it is
    /// selectable.
    fn row_at(&self, position: Point) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.item.selectable() && row.rect.contains(position))
    }

    /// The stagger the rows arrive on.
    fn stagger(&self, reduce: bool) -> Stagger {
        let base = Stagger::eased(CONTEXT_MENU_STAGGER, CONTEXT_MENU_ROW_ENTER, EASE_OUT);
        if reduce { base.collapsed() } else { base }
    }

    /// Rebuild the unfold driver when `reduce_motion` flips, preserving what the
    /// old one was doing.
    fn sync_motion(&mut self, reduce: bool) {
        if self.reduced == Some(reduce) {
            return;
        }
        let was_exiting = self.morph.phase() == PresencePhase::Exiting;
        self.reduced = Some(reduce);
        let base = Presence::symmetric(Ramp::eased(CONTEXT_MENU_MORPH, EASE_OUT));
        let mut next = if reduce { base.collapsed() } else { base };
        if was_exiting && !self.config.open {
            next.set_open(true);
        }
        next.set_open(self.config.open);
        self.morph = next;
    }
}

impl<State: 'static> View<State> for ContextMenuPanelView<State> {
    type Element = ContextMenuPanelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ContextMenuPanelWidget {
        let config = self.config.borrow().clone();
        let mut morph = Presence::symmetric(Ramp::eased(CONTEXT_MENU_MORPH, EASE_OUT));
        morph.set_open(config.open);
        ContextMenuPanelWidget {
            rows: self.items.iter().cloned().map(Row::new).collect(),
            config,
            morph,
            reduced: None,
            active: None,
            armed: None,
            pill: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0),
            pill_from: Rect::ZERO,
            pill_to: Rect::ZERO,
            pill_alpha: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0),
            entered_at: None,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ContextMenuPanelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.rows.len() != self.items.len() {
            element.rows = self.items.iter().cloned().map(Row::new).collect();
            element.active = None;
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (row, item) in element.rows.iter_mut().zip(&self.items) {
                if row.sync(item) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        let config = self.config.borrow().clone();
        element.config.anchor = config.anchor.clone();
        if element.config != config {
            element.config.open = config.open;
            element.morph.set_open(config.open);
            if config.open {
                // A fresh open episode: the stagger is timed from the frame the
                // panel next paints, and nothing is active until the pointer or
                // the keyboard says so.
                element.entered_at = None;
                element.active = None;
                element.armed = None;
                element.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0);
                element.pill_alpha = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0);
                element.pill_from = Rect::ZERO;
                element.pill_to = Rect::ZERO;
            }
            flags |= ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_select = erase_callback_arg(&self.on_select);
        flags
    }

    fn teardown(&self, _element: &mut ContextMenuPanelWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Row {
    fn new(item: ContextMenuItem) -> Self {
        Row {
            label: LabelRun::new(item.label.clone()),
            shortcut: item.shortcut.clone().map(LabelRun::new),
            item,
            rect: Rect::ZERO,
        }
    }

    /// Adopt `item`, reporting whether anything the layout depends on changed.
    fn sync(&mut self, item: &ContextMenuItem) -> bool {
        let mut changed = self.label.set_content(item.label.clone());
        match (&mut self.shortcut, &item.shortcut) {
            (Some(run), Some(text)) => changed |= run.set_content(text.clone()),
            (slot @ None, Some(text)) => {
                *slot = Some(LabelRun::new(text.clone()));
                changed = true;
            }
            (slot @ Some(_), None) => {
                *slot = None;
                changed = true;
            }
            (None, None) => {}
        }
        changed |= self.item.kind != item.kind
            || self.item.disabled != item.disabled
            || self.item.tone != item.tone;
        self.item = item.clone();
        changed
    }
}

impl Widget for ContextMenuPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let item_style = item_style(theme);
        let shortcut_style = shortcut_style(theme);
        let section_style = section_style(theme);

        let mut widest: f64 = 0.0;
        for row in &mut self.rows {
            let style = match row.item.kind {
                RowKind::Item => &item_style,
                RowKind::Label => &section_style,
                RowKind::Separator => continue,
            };
            let label = row.label.layout(ctx, style);
            let shortcut = row
                .shortcut
                .as_mut()
                .map(|run| run.layout(ctx, &shortcut_style).width)
                .unwrap_or(0.0);
            let gap = if shortcut > 0.0 {
                CONTEXT_MENU_SHORTCUT_GAP
            } else {
                0.0
            };
            widest = widest.max(label.width + gap + shortcut + CONTEXT_MENU_ITEM_PADDING_X * 2.0);
        }
        let width = (widest + CONTEXT_MENU_PADDING * 2.0)
            .max(CONTEXT_MENU_MIN_WIDTH)
            .min(bc.max().width.max(CONTEXT_MENU_MIN_WIDTH));

        let mut y = CONTEXT_MENU_PADDING;
        for row in &mut self.rows {
            let height = row.item.height();
            row.rect = Rect::new(
                CONTEXT_MENU_PADDING,
                y,
                width - CONTEXT_MENU_PADDING,
                y + height,
            );
            y += height;
        }
        bc.constrain(Size::new(width, y + CONTEXT_MENU_PADDING))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let chrome = resolve_panel(Theme::from_paint_ctx(ctx));
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        self.sync_motion(reduce);

        let now = ctx.frame_time();
        let progress = self.morph.advance(now);
        if self.morph.is_animating() {
            ctx.request_frame();
        }
        if !self.morph.is_visible() {
            self.entered_at = None;
            return;
        }
        let travelled = self.pill.advance(now);
        let faded = self.pill_alpha.advance(now);
        if travelled || faded {
            ctx.request_frame();
        }

        let panel = Rect::from_origin_size(Point::ORIGIN, ctx.size());
        // `collapsedClip`: a 2×`half` seed square centred on the press point,
        // clamped into the panel. The anchor is a zero-size rect at that point.
        let point = self.config.anchor.rect().origin() - ctx.origin().to_vec2();
        let seed = Rect::new(
            (point.x - CONTEXT_MENU_SEED).clamp(panel.x0, panel.x1),
            (point.y - CONTEXT_MENU_SEED).clamp(panel.y0, panel.y1),
            (point.x + CONTEXT_MENU_SEED).clamp(panel.x0, panel.x1),
            (point.y + CONTEXT_MENU_SEED).clamp(panel.y0, panel.y1),
        );
        let (rect, radius) = morph_rect(
            seed,
            CONTEXT_MENU_SEED_RADIUS,
            panel,
            style::RADIUS_XL,
            progress,
        );
        paint_panel(scene, ctx.origin(), rect, radius, chrome);
        scene.push_clip_rounded(ctx.origin() + rect.origin().to_vec2(), rect.size(), radius);
        self.paint_rows(ctx, scene, chrome, now, reduce);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.config.open {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Pointer(p) => {
                if !inside(p.position, ctx.size()) {
                    return EventResult::Ignored;
                }
                match p.phase {
                    PointerPhase::Move => {
                        ctx.claim_hover();
                        if self.set_active(self.row_at(p.position)) {
                            ctx.request_redraw();
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        // Activation is armed-and-re-hit, not "wherever the Up
                        // lands": only a row a primary Down armed fires, and
                        // only when the release lands back on that same row —
                        // both geometric re-hit and identity match required to prevent
                        // a same-length item swap between Down and Up from activating
                        // a different action. A stray Up (no arming Down reached this
                        // widget) or a secondary button (which never arms below) does
                        // nothing.
                        if let Some((armed_index, armed_item)) = self.armed.take() {
                            let item_at_index = self.rows.get(armed_index).map(|r| &r.item);
                            let identity_match = item_at_index.is_some_and(|item| {
                                item.label == armed_item.label
                                    && item.tone == armed_item.tone
                                    && item.disabled == armed_item.disabled
                                    && item.kind == armed_item.kind
                            });
                            if self.row_at(p.position) == Some(armed_index) && identity_match {
                                (self.on_select)(ctx, armed_index);
                            }
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Down => {
                        // A press inside the panel is the panel's: swallowed so
                        // the host's light dismiss never sees it, and claimed
                        // for focus so the arrow keys reach these rows. The
                        // host claims focus for *itself* on the same press, and
                        // a focus-routed key it holds is never handed on to its
                        // content — both pods sit on one chain and the deeper
                        // claim wins the descent, which is the only way a key
                        // reaches a widget inside an anchored panel.
                        //
                        // Only a primary press arms a row for activation — a
                        // secondary press still claims focus (it is the panel's
                        // event either way) but starts no activation.
                        // Every primary press re-arms: a press that misses a
                        // selectable row clears a stale arm, so a lost release
                        // can never leave one behind to fire later.
                        if presses(p) {
                            self.armed = self
                                .row_at(p.position)
                                .map(|index| (index, self.rows[index].item.clone()));
                        }
                        ctx.request_focus();
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        // Never touches real state beyond clearing the arm —
                        // see the Cancel-never-mutates-state rule.
                        if self.armed.take().is_none() {
                            return EventResult::Ignored;
                        }
                        EventResult::Handled
                    }
                }
            }
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowDown) => 1,
                    Key::Named(NamedKey::ArrowUp) => -1,
                    _ => 0,
                };
                if step != 0 {
                    if self.set_active(self.step_active(step)) {
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                if is_activation_key(key)
                    && let Some(index) = self.active
                {
                    // Re-check that the active row is still selectable before firing.
                    // It may have become disabled or been replaced since navigation.
                    if self
                        .rows
                        .get(index)
                        .map(|r| r.item.selectable())
                        .unwrap_or(false)
                    {
                        (self.on_select)(ctx, index);
                    } else {
                        // `set_active` owns the transition — writing
                        // `self.active` first would trip its own diff guard
                        // and leave the pill painted on a dead row.
                        if self.set_active(None) {
                            ctx.request_redraw();
                        }
                    }
                    return EventResult::Handled;
                }
                // Escape and everything else is the host's.
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !self.config.open {
            return;
        }
        ctx.push_container(
            Role::Menu,
            |_| {},
            |ctx| {
                for row in &self.rows {
                    if row.item.kind != RowKind::Item {
                        continue;
                    }
                    let label = row.label.content().to_string();
                    let disabled = row.item.disabled;
                    ctx.push_node(Role::MenuItem, |node| {
                        node.set_label(label.as_str());
                        if disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }
}

impl ContextMenuPanelWidget {
    /// Paint the pill and every row, each on its own stagger slot.
    fn paint_rows(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        chrome: PanelChrome,
        now: FrameTime,
        reduce: bool,
    ) {
        let origin = ctx.origin();
        let entered = *self.entered_at.get_or_insert(now);
        let elapsed = now.saturating_sub(entered);
        let stagger = self.stagger(reduce);
        let count = self.rows.len();
        if !stagger.is_settled(elapsed, count) {
            ctx.request_frame();
        }

        // The shared active pill, under the rows — `layoutId` in one rect.
        if self.active.is_some() || self.pill_alpha.value() > 0.0 {
            let tone = self
                .active
                .and_then(|i| self.rows.get(i))
                .map_or(ContextMenuTone::Default, |row| row.item.tone);
            let (base, alpha) = match tone {
                ContextMenuTone::Default => (chrome.ink, CONTEXT_MENU_PILL_ALPHA),
                ContextMenuTone::Destructive => (chrome.danger_ink, CONTEXT_MENU_DANGER_PILL_ALPHA),
            };
            let pill = self.pill_rect();
            if pill.width() > 0.0 && pill.height() > 0.0 {
                scene.fill_rounded_rect(
                    origin + pill.origin().to_vec2(),
                    pill.size(),
                    style::RADIUS_LG,
                    style::with_alpha(base, alpha * self.pill_alpha.value().clamp(0.0, 1.0) as f32),
                );
            }
        }

        for (index, row) in self.rows.iter().enumerate() {
            let revealed = stagger.revealed(elapsed, index, count);
            if revealed <= 0.0 {
                continue;
            }
            let lift = Vec2::new(0.0, (1.0 - revealed) * CONTEXT_MENU_ROW_LIFT);
            let layered = revealed < 1.0;
            if layered {
                scene.push_layer(
                    origin + row.rect.origin().to_vec2() + lift,
                    row.rect.size(),
                    revealed as f32,
                );
            }
            row.paint(origin + lift, chrome, scene);
            if layered {
                scene.pop_layer();
            }
        }
    }
}

impl Row {
    /// Paint this row's rule or its label/shortcut pair.
    fn paint(&self, origin: Point, chrome: PanelChrome, scene: &mut dyn PaintScene) {
        match self.item.kind {
            RowKind::Separator => {
                let y = self.rect.y0 + CONTEXT_MENU_SEPARATOR_MARGIN;
                scene.fill_rect(
                    origin + Vec2::new(self.rect.x0 - CONTEXT_MENU_SEPARATOR_BLEED, y),
                    Size::new(
                        self.rect.width() + CONTEXT_MENU_SEPARATOR_BLEED * 2.0,
                        style::BORDER_WIDTH,
                    ),
                    chrome.border,
                );
            }
            RowKind::Label => {
                let ink = chrome.dim_ink;
                let text = self.label.size();
                let at = origin
                    + Vec2::new(
                        self.rect.x0 + CONTEXT_MENU_ITEM_PADDING_X,
                        self.rect.y1 - CONTEXT_MENU_SEPARATOR_MARGIN - text.height,
                    );
                self.label.paint(at, ink, scene);
            }
            RowKind::Item => {
                let base = match self.item.tone {
                    ContextMenuTone::Default => chrome.ink,
                    ContextMenuTone::Destructive => chrome.danger_ink,
                };
                let ink =
                    style::disabled_tint(base, self.item.disabled, CONTEXT_MENU_DISABLED_OPACITY);
                let text = self.label.size();
                let baseline = self.rect.y0 + (self.rect.height() - text.height) / 2.0;
                self.label.paint(
                    origin + Vec2::new(self.rect.x0 + CONTEXT_MENU_ITEM_PADDING_X, baseline),
                    ink,
                    scene,
                );
                if let Some(shortcut) = &self.shortcut {
                    let size = shortcut.size();
                    let dim = style::disabled_tint(
                        chrome.dim_ink,
                        self.item.disabled,
                        CONTEXT_MENU_DISABLED_OPACITY,
                    );
                    shortcut.paint(
                        origin
                            + Vec2::new(
                                self.rect.x1 - CONTEXT_MENU_ITEM_PADDING_X - size.width,
                                self.rect.y0 + (self.rect.height() - size.height) / 2.0,
                            ),
                        dim,
                        scene,
                    );
                }
            }
        }
    }
}

// ---- The trigger -----------------------------------------------------------

/// Wrap `child` as a context-menu trigger region: a secondary press inside it
/// reports `on_open_change(state, true)` and publishes the press point into
/// `anchor`.
///
/// Transparent in every other respect — it lays out, paints, routes and
/// publishes the semantics of `child` unchanged, and the whole primary gesture
/// passes straight through to whatever is inside it.
pub fn context_menu_trigger<State: 'static, V: View<State>>(
    anchor: &OverlayAnchor,
    child: V,
) -> ContextMenuTriggerView<State> {
    ContextMenuTriggerView {
        child: any(child),
        anchor: anchor.clone(),
        on_open_change: Rc::new(|_, _| {}),
    }
}

/// A declarative context-menu trigger region. See [`context_menu_trigger`].
pub struct ContextMenuTriggerView<State: 'static> {
    child: AnyView<State>,
    anchor: OverlayAnchor,
    on_open_change: OnOpenChange<State>,
}

impl<State: 'static> ContextMenuTriggerView<State> {
    /// Set the open-change callback: `true` on a secondary press inside.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// The retained widget for a [`ContextMenuTriggerView`].
pub struct ContextMenuTriggerWidget {
    child: ChildPod,
    anchor: OverlayAnchor,
    /// The latched press point, in this widget's own space, published to the
    /// anchor on the next paint.
    point: Option<Point>,
    on_open_change: ErasedArgCallback<bool>,
}

impl ContextMenuTriggerWidget {
    /// The press point this trigger will publish, in its own space.
    pub fn press_point(&self) -> Option<Point> {
        self.point
    }
}

impl<State: 'static> View<State> for ContextMenuTriggerView<State> {
    type Element = ContextMenuTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ContextMenuTriggerWidget {
        ContextMenuTriggerWidget {
            child: build_child(&self.child, ctx),
            anchor: self.anchor.clone(),
            point: None,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ContextMenuTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        element.anchor = self.anchor.clone();
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut ContextMenuTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ContextMenuTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::origin` is absolute window space — the one read that turns a
        // widget-local press point into the window-space anchor the host needs.
        let origin = ctx.origin();
        match self.point {
            Some(p) => self
                .anchor
                .set(Rect::from_origin_size(origin + p.to_vec2(), Size::ZERO)),
            None => self.anchor.set(Rect::from_origin_size(origin, ctx.size())),
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A secondary press is claimed before the child sees it — the region owns
        // that button, and no control in this catalog does anything with it —
        // while every other pass, the whole primary gesture included, is the
        // child's.
        if let InputEvent::Pointer(p) = event
            && p.button == PointerButton::Secondary
            && p.phase == PointerPhase::Down
            && inside(p.position, ctx.size())
        {
            self.point = Some(p.position);
            // Focus is what routes Escape and the arrow keys to the menu.
            ctx.request_focus();
            (self.on_open_change)(ctx, true);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{
        Recorder, WINDOW, escape, ft_ms, light, pointer, reduced, secondary,
    };
    use frust::authoring::text::TextContext;
    use frust::{Color, SizedBox};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct App {
        open: bool,
        opens: Vec<bool>,
        selected: Vec<usize>,
        /// Rebuild with `Back` disabled — a row going dead under the pill.
        disable_back: bool,
        /// Rebuild with `Back` replaced by another item at the same index.
        rename_back: bool,
    }

    /// The trigger region, inset inside the window so a published anchor point
    /// proves it is absolute rather than region-local.
    const INSET: f64 = 24.0;
    const REGION: Size = Size::new(520.0, 400.0);

    /// The fixture rows, with the state's mutations applied.
    fn s_items(s: &App) -> Vec<ContextMenuItem> {
        let mut list = items();
        if s.disable_back {
            list[1] = context_menu_item("Back").disabled(true);
        }
        if s.rename_back {
            list[1] = context_menu_item("Backwards");
        }
        list
    }

    fn items() -> Vec<ContextMenuItem> {
        vec![
            context_menu_label("Actions"),
            context_menu_item("Back"),
            context_menu_item("Forward").disabled(true),
            context_menu_separator(),
            context_menu_item("Reload").shortcut("⌘R"),
            context_menu_item("Delete").tone(ContextMenuTone::Destructive),
        ]
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        anchor: OverlayAnchor,
        theme: Theme,
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
                anchor: OverlayAnchor::new(),
                theme: theme.clone(),
            };
            h.root.set_theme(Box::new(theme));
            h.frame(0.0);
            h
        }

        fn frame(&mut self, ms: f64) {
            let anchor = self.anchor.clone();
            let mut logic = move |s: &mut App| {
                frust::Stack(vec![
                    any(frust::Padding(
                        frust::EdgeInsets::all(INSET),
                        context_menu_trigger(
                            &anchor,
                            SizedBox(Some(REGION.width), Some(REGION.height)),
                        )
                        .on_open_change(|s: &mut App, open| {
                            s.open = open;
                            s.opens.push(open);
                        }),
                    )),
                    any(
                        context_menu(s_items(s), |s: &mut App, index| s.selected.push(index))
                            .anchor(&anchor)
                            .open(s.open)
                            .on_open_change(|s: &mut App, open| {
                                s.open = open;
                                s.opens.push(open);
                            }),
                    ),
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

        /// Right-click at `(x, y)` in window space and settle the panel open.
        fn open_at(&mut self, x: f64, y: f64) {
            self.event(secondary(PointerPhase::Down, x, y));
            self.frame(0.0);
            self.frame(16.0);
            self.frame(2_000.0);
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    // ---- The trigger ------------------------------------------------------

    #[test]
    fn a_secondary_press_opens_the_menu_and_publishes_the_press_point() {
        let mut h = Harness::new();
        h.event(secondary(PointerPhase::Down, INSET + 40.0, INSET + 30.0));
        assert_eq!(h.state.opens, vec![true]);
        h.frame(0.0);
        let anchor = h.anchor.rect();
        assert_eq!(anchor.size(), Size::ZERO, "a point, not a box");
        assert_eq!(
            anchor.origin(),
            Point::new(INSET + 40.0, INSET + 30.0),
            "published in absolute window space"
        );
    }

    #[test]
    fn a_primary_press_passes_straight_through_the_trigger() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Down, INSET + 10.0, INSET + 10.0));
        assert!(
            h.state.opens.is_empty(),
            "only the secondary button opens it"
        );
    }

    #[test]
    fn before_any_press_the_trigger_publishes_its_own_box() {
        let h = Harness::new();
        assert_eq!(
            h.anchor.rect(),
            Rect::from_origin_size(Point::new(INSET, INSET), REGION)
        );
    }

    // ---- Placement and the unfold -----------------------------------------

    #[test]
    fn the_panel_opens_with_its_leading_top_corner_on_the_pointer() {
        let mut h = Harness::new();
        h.open_at(INSET + 40.0, INSET + 30.0);
        let rec = h.paint_at(2_000.0);
        let (origin, _, radius, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, r, _)| *r == style::RADIUS_XL)
            .expect("the settled panel");
        assert_eq!(origin, Point::new(INSET + 40.0, INSET + 30.0));
        assert_eq!(radius, style::RADIUS_XL);
    }

    #[test]
    fn the_first_frame_shows_only_the_seed_square_at_the_pointer() {
        let mut h = Harness::new();
        h.event(secondary(PointerPhase::Down, INSET + 40.0, INSET + 30.0));
        // Two frames: the first publishes the press point (a trigger learns its
        // own absolute origin only at paint), the second places the panel on it.
        h.frame(0.0);
        h.frame(0.0);
        // The unfold is timed from the paint that started it, so re-painting the
        // same frame reads it at zero.
        let rec = h.paint_at(0.0);
        let (origin, size, _, _) = *rec.rrects.first().expect("the unfolding panel");
        assert!(
            size.width <= CONTEXT_MENU_SEED * 2.0 + 1.0
                && size.height <= CONTEXT_MENU_SEED * 2.0 + 1.0,
            "the seed is 2x`half` on each axis: {size:?}"
        );
        // Centred on the press point, clamped into the panel — the panel's own
        // leading corner is the point, so the seed pins there.
        assert_eq!(origin, Point::new(INSET + 40.0, INSET + 30.0));
    }

    #[test]
    fn the_panel_is_clamped_inside_the_window_rather_than_flipped() {
        let mut h = Harness::new();
        // A press near the region's bottom-right corner, where a panel placed
        // below-trailing of the pointer would overflow the window: upstream
        // clamps, never flips.
        h.open_at(INSET + REGION.width - 4.0, INSET + REGION.height - 4.0);
        let rec = h.paint_at(2_000.0);
        let (origin, size, _, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, r, _)| *r == style::RADIUS_XL)
            .expect("the settled panel");
        assert!(
            origin.x + size.width <= WINDOW.width - crate::overlay::VIEWPORT_PADDING + 0.001,
            "clamped inside the padded window: {origin:?} {size:?}"
        );
        assert!(origin.y + size.height <= WINDOW.height - crate::overlay::VIEWPORT_PADDING + 0.001);
    }

    #[test]
    fn reduce_motion_shows_the_whole_panel_on_the_first_frame() {
        let mut h = Harness::themed(reduced());
        h.event(secondary(PointerPhase::Down, INSET + 40.0, INSET + 30.0));
        h.frame(0.0);
        let rec = h.paint_at(0.0);
        let (_, size, _, _) = *rec.rrects.first().expect("the panel");
        assert!(
            size.width >= CONTEXT_MENU_MIN_WIDTH,
            "no unfold at all: {size:?}"
        );
    }

    // ---- Rows -------------------------------------------------------------

    #[test]
    fn the_panel_is_at_least_the_upstream_minimum_and_stacks_every_row() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let rec = h.paint_at(2_000.0);
        let (_, size, _, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, r, _)| *r == style::RADIUS_XL)
            .expect("the panel");
        assert!(size.width >= CONTEXT_MENU_MIN_WIDTH);
        let expected: f64 =
            items().iter().map(ContextMenuItem::height).sum::<f64>() + CONTEXT_MENU_PADDING * 2.0;
        assert_eq!(size.height, expected);
    }

    #[test]
    fn every_row_paints_its_own_ink_and_the_separator_paints_a_rule() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let rec = h.paint_at(2_000.0);
        let theme = h.theme().clone();
        assert!(
            rec.inks.contains(&theme.scheme().error),
            "the destructive row is inked with `--destructive`"
        );
        assert!(
            rec.inks.contains(&theme.scheme().on_surface_variant),
            "the section label and the shortcut take the dimmed role"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(_, s, c)| s.height == style::BORDER_WIDTH
                    && *c == theme.scheme().outline_variant),
            "the separator is a one-pixel rule in the border role"
        );
    }

    #[test]
    fn a_disabled_row_is_dimmed_and_never_takes_the_pill() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        // Hover the disabled `Forward` row: nothing lights up.
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let disabled_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT * 1.5;
        h.event(pointer(PointerPhase::Move, INSET + 40.0, disabled_y));
        h.frame(2_016.0);
        let before = h.paint_at(2_032.0);
        assert!(
            !before
                .rrects
                .iter()
                .any(|(_, _, r, _)| *r == style::RADIUS_LG),
            "no pill over a disabled row"
        );
    }

    #[test]
    fn hovering_a_row_lights_the_pill_and_a_primary_press_release_selects_it() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        h.event(pointer(PointerPhase::Move, INSET + 40.0, back_y));
        h.frame(2_016.0);
        let rec = h.paint_at(2_500.0);
        assert!(
            rec.rrects.iter().any(|(_, _, r, _)| *r == style::RADIUS_LG),
            "the pill is a `rounded-lg` fill under the row"
        );
        // Activation is armed-and-re-hit: the Down that arms the row, then an
        // Up that lands back on it.
        h.event(pointer(PointerPhase::Down, INSET + 40.0, back_y));
        h.event(pointer(PointerPhase::Up, INSET + 40.0, back_y));
        assert_eq!(h.state.selected, vec![1], "the index the caller wrote");
    }

    #[test]
    fn a_stray_up_with_no_arming_down_selects_nothing() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        // No Down landed on the panel at all — a hover alone never arms a row.
        h.event(pointer(PointerPhase::Move, INSET + 40.0, back_y));
        h.frame(2_016.0);
        h.event(pointer(PointerPhase::Up, INSET + 40.0, back_y));
        assert!(h.state.selected.is_empty(), "a stray Up must not activate");
    }

    #[test]
    fn a_secondary_press_release_on_a_row_selects_nothing() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        h.event(secondary(PointerPhase::Down, INSET + 40.0, back_y));
        h.event(secondary(PointerPhase::Up, INSET + 40.0, back_y));
        assert!(
            h.state.selected.is_empty(),
            "a secondary press/release never arms or activates a row"
        );
    }

    #[test]
    fn a_release_that_drifts_off_the_armed_row_selects_nothing() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        // A row below `Back` — landing anywhere off `Back`'s rect is enough to
        // fail the re-hit, disabled or not.
        let elsewhere_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT * 1.5;
        h.event(pointer(PointerPhase::Down, INSET + 40.0, back_y));
        // The release lands on a different row than the one the Down armed.
        h.event(pointer(PointerPhase::Up, INSET + 40.0, elsewhere_y));
        assert!(
            h.state.selected.is_empty(),
            "a release off the armed row must not re-hit a different one"
        );
    }

    /// The `rounded-lg` pill in a paint, if any: its window-space origin and
    /// its color.
    fn pill_of(rec: &Recorder) -> Option<(Point, Color)> {
        let mut pills = rec
            .rrects
            .iter()
            .filter(|(_, _, r, _)| *r == style::RADIUS_LG);
        let pill = pills.next().map(|(o, _, _, c)| (*o, *c));
        assert!(pills.next().is_none(), "at most one pill per paint");
        pill
    }

    /// Focus the panel through a primary press on the (unselectable) section
    /// label, so the arrow keys reach the rows without arming anything.
    fn focus_panel(h: &mut Harness) {
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        h.event(pointer(
            PointerPhase::Down,
            INSET + 40.0,
            panel_top + CONTEXT_MENU_LABEL_HEIGHT / 2.0,
        ));
    }

    #[test]
    fn a_row_to_row_move_slides_the_pill_without_fading_it() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        focus_panel(&mut h);
        h.event(arrow_down());
        h.frame(2_016.0);
        h.frame(3_000.0);
        let (start, settled_color) =
            pill_of(&h.paint_at(3_000.0)).expect("the pill sits on `Back`");
        assert_eq!(settled_color.components[3], CONTEXT_MENU_PILL_ALPHA);

        // `Back` -> `Reload`, sampled across the whole spring.
        h.event(arrow_down());
        let mut last_y = start.y;
        for ms in [3_016.0, 3_060.0, 3_120.0, 3_200.0, 3_400.0, 5_000.0] {
            let (origin, color) =
                pill_of(&h.paint_at(ms)).expect("the pill never disappears mid-move");
            assert_eq!(
                color.components[3], CONTEXT_MENU_PILL_ALPHA,
                "at {ms}ms the pill slides at its resting alpha — a move is not a fade"
            );
            assert!(
                origin.y >= last_y,
                "at {ms}ms the pill only travels towards `Reload`"
            );
            last_y = origin.y;
        }
        assert!(
            last_y >= start.y + CONTEXT_MENU_ITEM_HEIGHT,
            "the pill arrived on a lower row"
        );
    }

    #[test]
    fn a_retarget_mid_travel_continues_from_the_displayed_rect_at_full_alpha() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        focus_panel(&mut h);
        h.event(arrow_down());
        h.frame(2_016.0);
        h.frame(3_000.0);
        let (start, _) = pill_of(&h.paint_at(3_000.0)).expect("the pill sits on `Back`");

        // `Back` -> `Reload`, then `Reload` -> `Delete` while still in flight.
        // A lane's clock starts on its first paint after the retarget, so
        // paint once to latch it before sampling mid-flight.
        h.event(arrow_down());
        h.paint_at(3_016.0);
        let (mid, _) = pill_of(&h.paint_at(3_100.0)).expect("mid-flight pill");
        assert!(
            mid.y > start.y,
            "the pill had left `Back` when the retarget landed"
        );
        h.event(arrow_down());
        let (resumed, _) = pill_of(&h.paint_at(3_101.0)).expect("pill right after the retarget");
        assert!(
            (resumed.y - mid.y).abs() < 1.0,
            "the retarget continues from the displayed rect, not from `Reload` or `Back`"
        );
        // `Delete` is destructive, so the pill takes the danger alpha the
        // moment it becomes the active row — a tone switch, not a fade.
        let mut last_y = resumed.y;
        for ms in [3_150.0, 3_250.0, 3_450.0, 5_000.0] {
            let (origin, color) = pill_of(&h.paint_at(ms)).expect("the pill never disappears");
            assert_eq!(
                color.components[3], CONTEXT_MENU_DANGER_PILL_ALPHA,
                "at {ms}ms: no fade"
            );
            assert!(
                origin.y >= last_y,
                "at {ms}ms the pill only travels towards `Delete`"
            );
            last_y = origin.y;
        }
        assert!(
            last_y >= start.y + 2.0 * CONTEXT_MENU_ITEM_HEIGHT,
            "the pill arrived two selectable rows down, on `Delete`"
        );
    }

    #[test]
    fn enter_on_a_row_that_became_unselectable_selects_nothing_and_hides_the_pill() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        focus_panel(&mut h);
        h.event(arrow_down());
        h.frame(2_016.0);
        h.frame(3_000.0);
        assert!(
            pill_of(&h.paint_at(3_000.0)).is_some(),
            "the pill sits on `Back`"
        );

        h.state.disable_back = true;
        h.frame(3_016.0);
        h.event(enter());
        assert!(
            h.state.selected.is_empty(),
            "a row that went dead never fires"
        );
        h.frame(3_032.0);
        h.frame(8_000.0);
        assert!(
            pill_of(&h.paint_at(8_000.0)).is_none(),
            "the pill faded out once its row was refused"
        );
    }

    #[test]
    fn a_primary_press_that_misses_a_row_clears_a_stale_arm() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        h.event(pointer(PointerPhase::Down, INSET + 40.0, back_y));
        // Its release was lost; the next primary press lands on the label.
        focus_panel(&mut h);
        h.event(pointer(PointerPhase::Up, INSET + 40.0, back_y));
        assert!(h.state.selected.is_empty(), "the miss disarmed `Back`");
    }

    #[test]
    fn a_same_index_item_swap_between_down_and_up_selects_nothing() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        h.event(pointer(PointerPhase::Down, INSET + 40.0, back_y));
        h.state.rename_back = true;
        h.frame(2_016.0);
        h.event(pointer(PointerPhase::Up, INSET + 40.0, back_y));
        assert!(
            h.state.selected.is_empty(),
            "the row under the release is not the row that was armed"
        );
    }

    #[test]
    fn the_arrow_keys_walk_the_selectable_rows_and_enter_activates_one() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        // The secondary press left focus on the *trigger*; the host claims it on
        // the first press landing on the panel, which is what routes the keys
        // into the rows (see the module docs).
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        h.event(pointer(
            PointerPhase::Down,
            INSET + 40.0,
            panel_top + CONTEXT_MENU_LABEL_HEIGHT / 2.0,
        ));
        h.event(arrow_down());
        h.event(arrow_down());
        h.frame(2_016.0);
        // `Back` then `Reload` — `Forward` is disabled and the separator and the
        // section label are not selectable.
        h.event(enter());
        assert_eq!(h.state.selected, vec![4]);
    }

    #[test]
    fn a_press_inside_the_panel_never_reaches_the_hosts_light_dismiss() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        h.state.opens.clear();
        let panel_top = INSET + 10.0 + CONTEXT_MENU_PADDING;
        let back_y = panel_top + CONTEXT_MENU_LABEL_HEIGHT + CONTEXT_MENU_ITEM_HEIGHT / 2.0;
        h.event(pointer(PointerPhase::Down, INSET + 40.0, back_y));
        assert!(
            h.state.opens.is_empty(),
            "the panel swallowed its own press"
        );
    }

    #[test]
    fn a_press_outside_the_panel_and_escape_each_close_it_once() {
        let mut h = Harness::new();
        h.open_at(INSET + 10.0, INSET + 10.0);
        h.state.opens.clear();
        h.event(pointer(
            PointerPhase::Down,
            WINDOW.width - 4.0,
            WINDOW.height - 4.0,
        ));
        assert_eq!(h.state.opens, vec![false]);
        h.frame(2_016.0);
        h.state.opens.clear();
        h.event(escape());
        assert!(
            h.state.opens.is_empty(),
            "a closed host dismisses on nothing"
        );
    }

    fn arrow_down() -> InputEvent {
        key(Key::Named(NamedKey::ArrowDown))
    }

    fn enter() -> InputEvent {
        key(Key::Named(NamedKey::Enter))
    }

    fn key(k: Key) -> InputEvent {
        InputEvent::Key(frust::authoring::KeyEvent {
            key: k,
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        })
    }
}
