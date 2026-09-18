//! Ports beUI's `swipeable-list` composed block —
//! `components/motion/swipeable-list.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `swipeable-list`: *"Mobile-style list rows that swipe left or right to
//! reveal contextual action buttons."*
//!
//! | upstream | here |
//! |---|---|
//! | list `flex w-full flex-col gap-2` | [`SWIPEABLE_LIST_ROW_GAP`] |
//! | row `relative isolate overflow-hidden rounded-2xl bg-muted` | the clipped row well, [`SWIPEABLE_LIST_RADIUS`] |
//! | surface `min-h-[72px] rounded-2xl border border-border bg-card px-4 py-3 shadow-sm` | [`SWIPEABLE_LIST_ROW_MIN_HEIGHT`], [`SWIPEABLE_LIST_ROW_PADDING_X`], [`SWIPEABLE_LIST_ROW_PADDING_Y`] |
//! | content `flex items-center gap-3`, title/description `mt-0.5` | [`SWIPEABLE_LIST_CONTENT_GAP`], [`SWIPEABLE_LIST_TITLE_GAP`] |
//! | action button `width: actionWidth`, inner `h-9 w-9 rounded-full` | [`SWIPEABLE_LIST_ACTION_WIDTH`], [`SWIPEABLE_LIST_ACTION_BOX`] |
//! | `group-active:scale-95` | [`SWIPEABLE_LIST_ACTION_PRESS_SCALE`] |
//! | `ACTION_TONE_CLASS` (neutral/primary/success/warning/danger) | [`SwipeActionTone`]'s resolved inks |
//! | `ROW_SETTLE {stiffness 560, damping 48, mass 0.82}` | [`SWIPEABLE_LIST_ROW_SETTLE`] |
//! | `dragElastic = 0.04`, `dragMomentum = false` | [`SWIPEABLE_LIST_DRAG_ELASTIC`] |
//! | `OPEN_DISTANCE_RATIO = 0.46`, `CLOSE_DISTANCE_RATIO = 0.72` | [`SWIPEABLE_LIST_OPEN_RATIO`], [`SWIPEABLE_LIST_CLOSE_RATIO`] |
//! | `revealThreshold = 34`, `actionWidth = 56` | [`SWIPEABLE_LIST_REVEAL_THRESHOLD`], [`SWIPEABLE_LIST_ACTION_WIDTH`] |
//! | `item.disabled` `opacity-60`, `drag={false}` | [`SWIPEABLE_LIST_DISABLED_OPACITY`] |
//! | `closeOnAction = true` | [`SwipeableListView::close_on_action`] |
//!
//! # Premise correction: there is no full-swipe dismiss
//!
//! The porting card asks for "full-swipe dismiss with presence exit".
//! `swipeable-list.tsx` has none. Its drag is bounded by
//! `dragConstraints={{ left: -rightWidth, right: leftWidth }}` — the row cannot
//! travel past its own action rail, so there is no "full swipe" to commit — and
//! nothing in the file removes a row, exits one, or reports a removal: the whole
//! `onDragEnd` decision tree resolves to exactly three outcomes, *open left*,
//! *open right* and *closed*. Removal is the consumer's, driven by an
//! [`SwipeableListView::on_action`] the consumer wired to a destructive action.
//!
//! So this port has three outcomes and no [`crate::motion::Presence`]. A
//! swipe-to-dismiss row does exist in this catalog — in
//! [`crate::components::animated_toast_stack`], whose source really does dismiss
//! past a threshold — and that is where the card's description belongs.
//!
//! # Premise correction: the thresholds are distance **and** velocity, and only
//! distance survives
//!
//! Upstream's release rules have two arms each: a distance arm and a velocity
//! arm (`OPEN_VELOCITY = 720`, `CLOSE_VELOCITY = 320`, plus a `FLING_DISTANCE`
//! floor so a flick still needs 14px of travel). frust's `PointerEvent` carries
//! no velocity — the gap [`crate::components::animated_toast_stack`] records for
//! its own swipe — so **only the distance arms are ported**, exactly:
//!
//! | upstream | here |
//! |---|---|
//! | open: `latest > max(revealThreshold, sideWidth * 0.46)` | ported verbatim |
//! | close: `\|latest\| < sideWidth * 0.72` | ported verbatim |
//! | open: `velocity > 720 && latest > 14` | **dropped** |
//! | close: `velocity < -320` | **dropped** |
//!
//! The consequence is one-directional and worth naming: a *fast, short* flick
//! that upstream would have opened springs back here instead, and a fast flick
//! back that upstream would have closed stays open until it is dragged back past
//! 72% of the rail. Nothing opens or closes that upstream would not have; the
//! gesture is simply stricter. [`SWIPEABLE_LIST_FLING_DISTANCE`] is kept as a
//! recorded constant so the arm can be restored the day a velocity reaches this
//! layer, without re-deriving it.
//!
//! # Controlled, one row open at a time
//!
//! `value` is a prop — an `(id, side)` pair or `None` — and the widget never
//! writes it: a release, an action and a drag that starts on a *different* row
//! all report the requested value through `on_value_change`. The
//! one-row-at-a-time invariant is upstream's `onDragStart`, which clears the
//! open row before the new drag begins, and is kept here.
//!
//! # Degradations against the web original
//!
//! - **No `content` / `renderItem` escape hatch.** Both replace the whole row
//!   body with arbitrary JSX. The row body here is `title`/`description`/`meta`
//!   strings plus a `leading` **view** (the child protocol
//!   [`crate::components::dock`] established), which is the same narrowing every
//!   text-bearing port in this catalog records. Action icons stay views, since
//!   the rail is nothing but icons.
//! - **Single-line text.** Upstream `truncate`s; a shaped run here is one line
//!   and is clipped to its column instead of ellipsised.
//! - **The rail is not keyboard-reachable.** Upstream gives each action button a
//!   real `tabIndex` that flips with the open side. The whole list is one widget
//!   here, so the actions are reached by opening a row and pressing them — the
//!   same call [`crate::blocks::expandable_action_bar`] makes for its own items.
//!   `inert` has no analogue either; a closed row's actions simply are not hit
//!   tested.
//! - **No `touch-pan-y` handoff.** Upstream marks the row so the page keeps the
//!   vertical scroll through a horizontal swipe. frust has no such declaration;
//!   a row captures the pointer on `Down` and a swipe that begins on a row
//!   belongs to the row.
//! - **No shadow tier on the surface.** `shadow-sm` is below the catalog's
//!   authored glass/panel shadow recipes and no token names it, so the surface
//!   takes its border and fill without one.

use std::rc::Rc;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, teardown_child, text::TextStyle,
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::motion::Ramp;
use crate::press::{Lane, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::BeuiTokens;

// ---- Metrics ---------------------------------------------------------------

/// One action's slot width, in logical px (`actionWidth = 56`).
pub const SWIPEABLE_LIST_ACTION_WIDTH: f64 = 56.0;

/// The travel a closed row must pass before a side opens at all, in logical px
/// (`revealThreshold = 34`) — the floor under the 46% rule.
pub const SWIPEABLE_LIST_REVEAL_THRESHOLD: f64 = 34.0;

/// A row's minimum height, in logical px (`min-h-[72px]`).
pub const SWIPEABLE_LIST_ROW_MIN_HEIGHT: f64 = 72.0;

/// A row surface's horizontal padding, in logical px (`px-4`).
pub const SWIPEABLE_LIST_ROW_PADDING_X: f64 = 16.0;

/// A row surface's vertical padding, in logical px (`py-3`).
pub const SWIPEABLE_LIST_ROW_PADDING_Y: f64 = 12.0;

/// The gap between two rows, in logical px (`gap-2`).
pub const SWIPEABLE_LIST_ROW_GAP: f64 = 8.0;

/// The gap between a row's leading slot, its text column and its meta note, in
/// logical px (`gap-3`).
pub const SWIPEABLE_LIST_CONTENT_GAP: f64 = 12.0;

/// The gap between a row's title and its description, in logical px
/// (`mt-0.5`).
pub const SWIPEABLE_LIST_TITLE_GAP: f64 = 2.0;

/// A row's corner radius, in logical px (`rounded-2xl`).
pub const SWIPEABLE_LIST_RADIUS: f64 = style::RADIUS_2XL;

/// An action's round hit mark, in logical px (`h-9 w-9`).
pub const SWIPEABLE_LIST_ACTION_BOX: f64 = 36.0;

/// The scale an action's mark presses to (`group-active:scale-95`).
pub const SWIPEABLE_LIST_ACTION_PRESS_SCALE: f64 = 0.95;

/// A disabled row's opacity (`opacity-60`).
pub const SWIPEABLE_LIST_DISABLED_OPACITY: f32 = 0.6;

// ---- Motion ----------------------------------------------------------------

/// The release spring: `ROW_SETTLE = { type: "spring", stiffness: 560, damping:
/// 48, mass: 0.82 }`, authored in the component rather than in `lib/ease.ts`.
///
/// Upstream calls it a *"distance-based release spring \[that\] keeps short
/// rebounds and full reveals feeling equally direct"* — noticeably stiffer than
/// any of [`crate::tokens::motion`]'s six, which is why the local number wins
/// over the shared token (the same call [`crate::components::tabs`] documents
/// for its own indicator spring). Its `restDelta`/`restSpeed` keys are Motion's
/// rest thresholds, which this crate carries at the driver rather than on the
/// spring — see [`crate::tokens::motion`]'s own note.
pub const SWIPEABLE_LIST_ROW_SETTLE: SpringDescription = SpringDescription {
    mass: 0.82,
    stiffness: 560.0,
    damping: 48.0,
};

/// How much of an over-drag past the rail's own width still moves the row
/// (`dragElastic = 0.04`) — almost nothing, which is the point.
pub const SWIPEABLE_LIST_DRAG_ELASTIC: f64 = 0.04;

/// The share of a side's rail a closed row must cross to open it
/// (`OPEN_DISTANCE_RATIO = 0.46`), floored at
/// [`SWIPEABLE_LIST_REVEAL_THRESHOLD`].
pub const SWIPEABLE_LIST_OPEN_RATIO: f64 = 0.46;

/// The share of a side's rail an open row must still hold to stay open
/// (`CLOSE_DISTANCE_RATIO = 0.72`).
pub const SWIPEABLE_LIST_CLOSE_RATIO: f64 = 0.72;

/// The travel a *flick* must still cover for upstream's velocity arm to open a
/// row (`FLING_DISTANCE = 14`).
///
/// Recorded, not used: the velocity arm it guards has no `PointerEvent`
/// counterpart here (see the [module docs](self)). It stays so the arm can be
/// restored without re-deriving the number.
pub const SWIPEABLE_LIST_FLING_DISTANCE: f64 = 14.0;

// ---- The view --------------------------------------------------------------

/// Which rail a row is opened toward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeSide {
    /// The leading rail, revealed by dragging **right**.
    Left,
    /// The trailing rail, revealed by dragging **left**.
    Right,
}

/// An action's colour role (`ACTION_TONE_CLASS`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwipeActionTone {
    /// `text-muted-foreground group-hover:text-foreground` — upstream's default.
    #[default]
    Neutral,
    /// `text-foreground`.
    Primary,
    /// `text-emerald-600` → the catalog's authored `--success`.
    Success,
    /// `text-amber-600` → the catalog's authored `--warning`.
    Warning,
    /// `text-destructive`.
    Danger,
}

/// Which row is open, and toward which rail (`SwipeableListValue`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwipeableListValue {
    /// The open row's id.
    pub id: String,
    /// The rail it is open toward.
    pub side: SwipeSide,
}

/// What [`SwipeableListView::on_action`] reports (`onAction`'s payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwipeActionEvent {
    /// The row the action belongs to.
    pub item_id: String,
    /// The action that was pressed.
    pub action_id: String,
    /// The rail it was revealed on.
    pub side: SwipeSide,
}

/// One action on a row's rail.
pub struct SwipeAction<State: 'static> {
    id: String,
    label: String,
    icon: AnyView<State>,
    tone: SwipeActionTone,
    disabled: bool,
}

/// An action identified by `id`, named `label` (its accessible name) and drawing
/// `icon`.
pub fn swipe_action<State: 'static, V: View<State>>(
    id: impl Into<String>,
    label: impl Into<String>,
    icon: V,
) -> SwipeAction<State> {
    SwipeAction {
        id: id.into(),
        label: label.into(),
        icon: any(icon),
        tone: SwipeActionTone::default(),
        disabled: false,
    }
}

impl<State: 'static> SwipeAction<State> {
    /// Pick this action's colour role.
    pub fn tone(mut self, tone: SwipeActionTone) -> Self {
        self.tone = tone;
        self
    }

    /// Make this action inert: dimmed and unpressable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// This action's id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// One list row.
pub struct SwipeableListItem<State: 'static> {
    id: String,
    title: Option<String>,
    description: Option<String>,
    meta: Option<String>,
    leading: Option<AnyView<State>>,
    left_actions: Vec<SwipeAction<State>>,
    right_actions: Vec<SwipeAction<State>>,
    disabled: bool,
}

/// A row identified by `id`, with nothing on it yet.
pub fn swipeable_row<State: 'static>(id: impl Into<String>) -> SwipeableListItem<State> {
    SwipeableListItem {
        id: id.into(),
        title: None,
        description: None,
        meta: None,
        leading: None,
        left_actions: Vec::new(),
        right_actions: Vec::new(),
        disabled: false,
    }
}

impl<State: 'static> SwipeableListItem<State> {
    /// The row's headline (`item.title`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The line under it (`item.description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The right-aligned note (`item.meta`).
    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// The view drawn at the row's leading edge (`item.leading` — an avatar or
    /// an icon, in upstream's own example).
    pub fn leading<V: View<State>>(mut self, leading: V) -> Self {
        self.leading = Some(any(leading));
        self
    }

    /// The actions revealed by dragging **right** (`item.leftActions`).
    pub fn left_actions(mut self, actions: Vec<SwipeAction<State>>) -> Self {
        self.left_actions = actions;
        self
    }

    /// The actions revealed by dragging **left** (`item.rightActions`).
    pub fn right_actions(mut self, actions: Vec<SwipeAction<State>>) -> Self {
        self.right_actions = actions;
        self
    }

    /// Make this row inert: dimmed and undraggable (`item.disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// This row's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Every child view this row owns, in the order the widget's pods hold
    /// them: the leading slot, then the left rail, then the right rail.
    fn child_views(&self) -> Vec<&AnyView<State>> {
        let mut views: Vec<&AnyView<State>> = Vec::new();
        if let Some(leading) = &self.leading {
            views.push(leading);
        }
        views.extend(self.left_actions.iter().map(|a| &a.icon));
        views.extend(self.right_actions.iter().map(|a| &a.icon));
        views
    }
}

/// A view-held open-row callback, erased on build.
type OnValueChange<State> = Rc<dyn Fn(&mut State, Option<SwipeableListValue>)>;

/// A view-held action callback, erased on build.
type OnAction<State> = Rc<dyn Fn(&mut State, SwipeActionEvent)>;

/// A declarative beUI swipeable list. See the [module docs](self).
pub struct SwipeableListView<State: 'static> {
    items: Vec<SwipeableListItem<State>>,
    value: Option<SwipeableListValue>,
    action_width: f64,
    reveal_threshold: f64,
    close_on_action: bool,
    on_value_change: OnValueChange<State>,
    on_action: Option<OnAction<State>>,
}

/// Create a swipeable list whose open row is `value`, reporting a requested open
/// row through `on_value_change` — a **controlled** component.
pub fn swipeable_list<State: 'static, F: Fn(&mut State, Option<SwipeableListValue>) + 'static>(
    items: Vec<SwipeableListItem<State>>,
    value: Option<SwipeableListValue>,
    on_value_change: F,
) -> SwipeableListView<State> {
    SwipeableListView {
        items,
        value,
        action_width: SWIPEABLE_LIST_ACTION_WIDTH,
        reveal_threshold: SWIPEABLE_LIST_REVEAL_THRESHOLD,
        close_on_action: true,
        on_value_change: Rc::new(on_value_change),
        on_action: None,
    }
}

impl<State: 'static> SwipeableListView<State> {
    /// Report a pressed action (`onAction`).
    pub fn on_action<F: Fn(&mut State, SwipeActionEvent) + 'static>(
        mut self,
        on_action: F,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }

    /// One action slot's width (`actionWidth`).
    pub fn action_width(mut self, width: f64) -> Self {
        self.action_width = width.max(0.0);
        self
    }

    /// The travel floor under the 46% open rule (`revealThreshold`).
    pub fn reveal_threshold(mut self, threshold: f64) -> Self {
        self.reveal_threshold = threshold.max(0.0);
        self
    }

    /// Whether pressing an action also closes the row (`closeOnAction`,
    /// default `true`).
    pub fn close_on_action(mut self, close: bool) -> Self {
        self.close_on_action = close;
        self
    }

    /// Every child view the list owns, row by row.
    fn child_views(&self) -> Vec<&AnyView<State>> {
        self.items
            .iter()
            .flat_map(|item| item.child_views())
            .collect()
    }
}

/// The resolved list palette.
struct SwipeableListColors {
    /// The row well the rail sits in (`bg-muted`).
    well: Color,
    /// The row surface (`bg-card`).
    surface: Color,
    /// Its hairline (`border-border`).
    border: Color,
    /// An action mark's hover disc (`group-hover:bg-background`).
    disc: Color,
    /// Title ink (`text-foreground`).
    ink: Color,
    /// Description/meta ink (`text-muted-foreground`).
    dim_ink: Color,
    /// `text-emerald-600` → `--success`.
    success: Color,
    /// `text-amber-600` → `--warning`.
    warning: Color,
    /// `text-destructive`.
    danger: Color,
}

impl SwipeableListColors {
    /// The ink an action of `tone` draws in.
    fn tone(&self, tone: SwipeActionTone) -> Color {
        match tone {
            SwipeActionTone::Neutral => self.dim_ink,
            SwipeActionTone::Primary => self.ink,
            SwipeActionTone::Success => self.success,
            SwipeActionTone::Warning => self.warning,
            SwipeActionTone::Danger => self.danger,
        }
    }
}

/// Resolve the palette, falling back to the vendored **light** table with no
/// theme threaded — the unthemed posture every component in this catalog takes.
fn resolve_colors(theme: Option<&Theme>) -> SwipeableListColors {
    let tokens = BeuiTokens::resolve(theme);
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            SwipeableListColors {
                well: s.surface_container_low,
                surface: s.surface_container,
                border: s.outline_variant,
                disc: s.surface,
                ink: s.on_surface,
                dim_ink: s.on_surface_variant,
                success: tokens.success,
                warning: tokens.warning,
                danger: s.error,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            SwipeableListColors {
                well: p.muted,
                surface: p.card,
                border: p.border,
                disc: p.background,
                ink: p.foreground,
                dim_ink: p.muted_foreground,
                success: tokens.success,
                warning: tokens.warning,
                danger: p.destructive,
            }
        }
    }
}

/// The row title's style (`text-sm font-medium`), in the theme's
/// `label_large` family.
fn title_style(theme: Option<&Theme>) -> TextStyle {
    themed_style(
        crate::text::label_style(style::TEXT_SM),
        ThemeTextType::LabelLarge,
        theme,
    )
}

/// The description style (`text-xs`, regular), in [`title_style`]'s family.
fn body_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        size: style::TEXT_XS as f32,
        weight: frust::authoring::text::FontWeight::REGULAR,
        ..title_style(theme)
    }
}

/// The meta style (`text-xs font-medium`), in [`title_style`]'s family.
fn meta_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        size: style::TEXT_XS as f32,
        ..title_style(theme)
    }
}

/// One retained action.
struct ActionEntry {
    id: String,
    label: String,
    tone: SwipeActionTone,
    disabled: bool,
}

impl ActionEntry {
    fn from_action<State: 'static>(action: &SwipeAction<State>) -> Self {
        ActionEntry {
            id: action.id.clone(),
            label: action.label.clone(),
            tone: action.tone,
            disabled: action.disabled,
        }
    }
}

/// One retained row.
struct RowState {
    id: String,
    title: Option<LabelRun>,
    description: Option<LabelRun>,
    meta: Option<LabelRun>,
    left: Vec<ActionEntry>,
    right: Vec<ActionEntry>,
    disabled: bool,
    /// Whether this row owns a leading pod.
    has_leading: bool,
    /// Index of this row's first pod in the widget's flat pod list.
    pod_base: usize,
    /// The row's own height, from the last layout.
    height: f64,
    /// The row's top edge in widget-local coordinates, from the last layout.
    top: f64,
    /// The settled/animating horizontal travel.
    travel: Lane,
    /// The live drag's travel, while a swipe is in flight.
    drag: Option<f64>,
}

impl RowState {
    fn from_item<State: 'static>(item: &SwipeableListItem<State>, pod_base: usize) -> Self {
        RowState {
            id: item.id.clone(),
            title: item.title.as_ref().map(LabelRun::new),
            description: item.description.as_ref().map(LabelRun::new),
            meta: item.meta.as_ref().map(LabelRun::new),
            left: item
                .left_actions
                .iter()
                .map(ActionEntry::from_action)
                .collect(),
            right: item
                .right_actions
                .iter()
                .map(ActionEntry::from_action)
                .collect(),
            disabled: item.disabled,
            has_leading: item.leading.is_some(),
            pod_base,
            height: SWIPEABLE_LIST_ROW_MIN_HEIGHT,
            top: 0.0,
            travel: Lane::at_rest(Ramp::spring(SWIPEABLE_LIST_ROW_SETTLE), 0.0),
            drag: None,
        }
    }

    /// The row's current horizontal displacement: the live drag if there is
    /// one, otherwise wherever the release spring has reached.
    fn x(&self) -> f64 {
        self.drag.unwrap_or_else(|| self.travel.value())
    }

    /// The row's box in widget-local coordinates.
    fn rect(&self, width: f64) -> Rect {
        Rect::from_origin_size(
            Point::new(0.0, self.top),
            Size::new(width.max(0.0), self.height),
        )
    }

    /// This row's leading-pod index, if it has one.
    fn leading_pod(&self) -> Option<usize> {
        self.has_leading.then_some(self.pod_base)
    }

    /// The pod index of action `index` on `side`.
    fn action_pod(&self, side: SwipeSide, index: usize) -> usize {
        let base = self.pod_base + usize::from(self.has_leading);
        match side {
            SwipeSide::Left => base + index,
            SwipeSide::Right => base + self.left.len() + index,
        }
    }

    /// The actions on `side`.
    fn actions(&self, side: SwipeSide) -> &[ActionEntry] {
        match side {
            SwipeSide::Left => &self.left,
            SwipeSide::Right => &self.right,
        }
    }
}

impl<State: 'static> View<State> for SwipeableListView<State> {
    type Element = SwipeableListWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SwipeableListWidget {
        let mut rows = Vec::with_capacity(self.items.len());
        let mut base = 0usize;
        for item in &self.items {
            rows.push(RowState::from_item(item, base));
            base += item.child_views().len();
        }
        let mut widget = SwipeableListWidget {
            rows,
            pods: self
                .child_views()
                .into_iter()
                .map(|view| build_child(view, ctx))
                .collect(),
            value: self.value.clone(),
            action_width: self.action_width,
            reveal_threshold: self.reveal_threshold,
            close_on_action: self.close_on_action,
            width: 0.0,
            armed: None,
            hovered: None,
            on_value_change: erase_callback_arg(&self.on_value_change),
            on_action: self.on_action.as_ref().map(erase_callback_arg),
        };
        widget.retarget_rows();
        for row in &mut widget.rows {
            row.travel.snap();
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwipeableListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        element.on_action = self.on_action.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        let prev_views = prev.child_views();
        let views = self.child_views();
        let same_shape = prev.items.len() == self.items.len()
            && prev_views.len() == views.len()
            && prev.items.iter().zip(self.items.iter()).all(|(a, b)| {
                a.id == b.id
                    && a.leading.is_some() == b.leading.is_some()
                    && a.left_actions.len() == b.left_actions.len()
                    && a.right_actions.len() == b.right_actions.len()
            });

        if same_shape {
            for (pod, (previous, next)) in element
                .pods
                .iter_mut()
                .zip(prev_views.into_iter().zip(views))
            {
                flags |= rebuild_child(next, previous, pod, ctx);
            }
            for (row, item) in element.rows.iter_mut().zip(self.items.iter()) {
                if row.sync(item) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        } else {
            // The row or rail shape changed: every pod is rebuilt from scratch,
            // and any live gesture is dropped with the rows it belonged to.
            for (view, pod) in prev_views.into_iter().zip(element.pods.iter_mut()) {
                teardown_child(view, pod, ctx);
            }
            let mut rows = Vec::with_capacity(self.items.len());
            let mut base = 0usize;
            for item in &self.items {
                rows.push(RowState::from_item(item, base));
                base += item.child_views().len();
            }
            element.rows = rows;
            element.pods = views
                .into_iter()
                .map(|view| build_child(view, ctx))
                .collect();
            element.armed = None;
            element.hovered = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.action_width != self.action_width
            || element.reveal_threshold != self.reveal_threshold
        {
            element.action_width = self.action_width;
            element.reveal_threshold = self.reveal_threshold;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.close_on_action = self.close_on_action;

        if prev.value != self.value {
            element.value = self.value.clone();
            element.retarget_rows();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SwipeableListWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.child_views().into_iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl RowState {
    /// Adopt `item`'s text and flags, reporting whether a re-measure is owed.
    fn sync<State: 'static>(&mut self, item: &SwipeableListItem<State>) -> bool {
        let mut changed = sync_run(&mut self.title, item.title.as_ref());
        changed |= sync_run(&mut self.description, item.description.as_ref());
        changed |= sync_run(&mut self.meta, item.meta.as_ref());
        changed |= self.disabled != item.disabled;
        self.disabled = item.disabled;
        for (entry, action) in self.left.iter_mut().zip(item.left_actions.iter()) {
            entry.id = action.id.clone();
            entry.label = action.label.clone();
            entry.tone = action.tone;
            entry.disabled = action.disabled;
        }
        for (entry, action) in self.right.iter_mut().zip(item.right_actions.iter()) {
            entry.id = action.id.clone();
            entry.label = action.label.clone();
            entry.tone = action.tone;
            entry.disabled = action.disabled;
        }
        changed
    }
}

/// Apply an optional label to an optional cached run, re-shaping only on a real
/// change.
fn sync_run(run: &mut Option<LabelRun>, text: Option<&String>) -> bool {
    match (run.as_mut(), text) {
        (Some(existing), Some(text)) => existing.set_content(text.clone()),
        (None, Some(text)) => {
            *run = Some(LabelRun::new(text.clone()));
            true
        }
        (Some(_), None) => {
            *run = None;
            true
        }
        (None, None) => false,
    }
}

/// What a `Down` armed on a row.
#[derive(Clone, Debug, PartialEq)]
struct Armed {
    /// The row's id, not its index — the list can move under a live press.
    id: String,
    /// The action it armed, when the press landed on a revealed one.
    action: Option<(SwipeSide, usize)>,
    /// Where the press started, in widget-local coordinates.
    start: Point,
    /// The row's travel when the press started.
    start_x: f64,
    /// Whether the pointer has moved far enough for this to be a drag.
    dragging: bool,
}

/// The retained widget for a [`SwipeableListView`].
pub struct SwipeableListWidget {
    rows: Vec<RowState>,
    /// Every row's child views, flattened: leading, then left rail, then right
    /// rail, row by row.
    pods: Vec<ChildPod>,
    /// The app-confirmed open row.
    value: Option<SwipeableListValue>,
    action_width: f64,
    reveal_threshold: f64,
    close_on_action: bool,
    /// The list's resolved width, from the last layout.
    width: f64,
    /// The press in flight.
    armed: Option<Armed>,
    /// The revealed action under the pointer, if any.
    hovered: Option<(usize, SwipeSide, usize)>,
    on_value_change: ErasedArgCallback<Option<SwipeableListValue>>,
    on_action: Option<ErasedArgCallback<SwipeActionEvent>>,
}

impl SwipeableListWidget {
    /// The row holding `id`.
    fn row_of(&self, id: &str) -> Option<usize> {
        self.rows.iter().position(|row| row.id == id)
    }

    /// Which side row `index` is open toward, per the confirmed value.
    fn open_side(&self, index: usize) -> Option<SwipeSide> {
        let value = self.value.as_ref()?;
        (self.rows.get(index)?.id == value.id).then_some(value.side)
    }

    /// The width of row `index`'s `side` rail.
    fn rail_width(&self, index: usize, side: SwipeSide) -> f64 {
        self.rows.get(index).map_or(0.0, |row| {
            row.actions(side).len() as f64 * self.action_width
        })
    }

    /// Row `index`'s resting travel: `+leftWidth` open left, `−rightWidth` open
    /// right, `0` closed (`targetX`).
    fn target_x(&self, index: usize) -> f64 {
        match self.open_side(index) {
            Some(SwipeSide::Left) => self.rail_width(index, SwipeSide::Left),
            Some(SwipeSide::Right) => -self.rail_width(index, SwipeSide::Right),
            None => 0.0,
        }
    }

    /// Re-aim every row's release spring at the travel the confirmed value calls
    /// for, starting from wherever it is displaying.
    fn retarget_rows(&mut self) {
        for index in 0..self.rows.len() {
            let target = self.target_x(index);
            let row = &mut self.rows[index];
            if row.travel.target() == target {
                continue;
            }
            row.travel = Lane::at_rest(Ramp::spring(SWIPEABLE_LIST_ROW_SETTLE), row.x());
            row.travel.retarget(target);
            row.drag = None;
        }
    }

    /// Clamp `raw` into the row's own rail bounds, letting
    /// [`SWIPEABLE_LIST_DRAG_ELASTIC`] of the excess through
    /// (`dragConstraints` + `dragElastic`).
    fn constrain(&self, index: usize, raw: f64) -> f64 {
        let left = self.rail_width(index, SwipeSide::Left);
        let right = -self.rail_width(index, SwipeSide::Right);
        if raw > left {
            left + (raw - left) * SWIPEABLE_LIST_DRAG_ELASTIC
        } else if raw < right {
            right + (raw - right) * SWIPEABLE_LIST_DRAG_ELASTIC
        } else {
            raw
        }
    }

    /// The side a released row settles toward, from its travel alone — every
    /// distance arm of upstream's `onDragEnd`, with the velocity arms dropped
    /// (see the [module docs](self)).
    fn release_side(&self, index: usize, latest: f64) -> Option<SwipeSide> {
        let left_width = self.rail_width(index, SwipeSide::Left);
        let right_width = self.rail_width(index, SwipeSide::Right);
        match self.open_side(index) {
            Some(SwipeSide::Left) => {
                (latest >= left_width * SWIPEABLE_LIST_CLOSE_RATIO).then_some(SwipeSide::Left)
            }
            Some(SwipeSide::Right) => (latest.abs() >= right_width * SWIPEABLE_LIST_CLOSE_RATIO)
                .then_some(SwipeSide::Right),
            None => {
                let left_open = self
                    .reveal_threshold
                    .max(left_width * SWIPEABLE_LIST_OPEN_RATIO);
                let right_open = self
                    .reveal_threshold
                    .max(right_width * SWIPEABLE_LIST_OPEN_RATIO);
                if left_width > 0.0 && latest > 0.0 && latest > left_open {
                    Some(SwipeSide::Left)
                } else if right_width > 0.0 && latest < 0.0 && latest < -right_open {
                    Some(SwipeSide::Right)
                } else {
                    None
                }
            }
        }
    }

    /// Action `action`'s slot on row `index`'s `side` rail, in widget-local
    /// coordinates. The rail does not move with the row: it is the well the
    /// surface slides off.
    fn action_rect(&self, index: usize, side: SwipeSide, action: usize) -> Option<Rect> {
        let row = self.rows.get(index)?;
        if action >= row.actions(side).len() {
            return None;
        }
        let x = match side {
            SwipeSide::Left => action as f64 * self.action_width,
            SwipeSide::Right => {
                self.width - self.rail_width(index, SwipeSide::Right)
                    + action as f64 * self.action_width
            }
        };
        Some(Rect::from_origin_size(
            Point::new(x, row.top),
            Size::new(self.action_width, row.height),
        ))
    }

    /// Row `index`'s surface box, displaced by its current travel.
    fn surface_rect(&self, index: usize) -> Option<Rect> {
        let row = self.rows.get(index)?;
        Some(row.rect(self.width) + frust::authoring::Vec2::new(row.x(), 0.0))
    }

    /// What a widget-local `pos` lands on: a row, and the revealed action under
    /// it if the pointer is past the surface on the open side.
    fn hit(&self, pos: Point) -> Option<(usize, Option<(SwipeSide, usize)>)> {
        let index = (0..self.rows.len()).find(|i| self.rows[*i].rect(self.width).contains(pos))?;
        if let Some(side) = self.open_side(index)
            && let Some(surface) = self.surface_rect(index)
            && !surface.contains(pos)
        {
            let count = self.rows[index].actions(side).len();
            for action in 0..count {
                if self
                    .action_rect(index, side, action)
                    .is_some_and(|rect| rect.contains(pos))
                    && !self.rows[index].actions(side)[action].disabled
                {
                    return Some((index, Some((side, action))));
                }
            }
        }
        Some((index, None))
    }

    /// Report the open row `next` asks for (never assigning it locally).
    fn request_value(&mut self, ctx: &mut EventCtx, next: Option<SwipeableListValue>) {
        if self.value == next {
            return;
        }
        (self.on_value_change)(ctx, next);
    }

    /// Advance every release spring to `now`, returning whether anything moves.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        let mut moving = false;
        for row in &mut self.rows {
            if reduce_motion {
                row.travel.snap();
            }
            moving |= row.travel.advance(now);
        }
        moving
    }
}

impl Widget for SwipeableListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let title = title_style(theme);
        let body = body_style(theme);
        let meta = meta_style(theme);

        self.width = crate::overlay::finite_or_zero(bc.max().width);
        let leading_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(self.width, SWIPEABLE_LIST_ROW_MIN_HEIGHT),
        );
        let action_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(SWIPEABLE_LIST_ACTION_BOX, SWIPEABLE_LIST_ACTION_BOX),
        );

        let mut y = 0.0;
        for index in 0..self.rows.len() {
            // Measure the row's own text first, so its height is known before
            // anything is placed inside it.
            let (text_height, leading_size) = {
                let row = &mut self.rows[index];
                let title_h = row
                    .title
                    .as_mut()
                    .map_or(0.0, |r| r.layout(ctx, &title).height);
                let description_h = row
                    .description
                    .as_mut()
                    .map(|r| r.layout(ctx, &body).height);
                if let Some(run) = row.meta.as_mut() {
                    run.layout(ctx, &meta);
                }
                let text = title_h + description_h.map_or(0.0, |h| SWIPEABLE_LIST_TITLE_GAP + h);
                (text, row.leading_pod())
            };
            let leading = match leading_size {
                Some(pod_index) => self.pods[pod_index].layout_child(ctx, &leading_bc),
                None => Size::ZERO,
            };
            let content = text_height.max(leading.height);
            let height =
                SWIPEABLE_LIST_ROW_MIN_HEIGHT.max(content + SWIPEABLE_LIST_ROW_PADDING_Y * 2.0);
            self.rows[index].height = height;
            self.rows[index].top = y;

            if let Some(pod_index) = self.rows[index].leading_pod() {
                let at = Point::new(
                    SWIPEABLE_LIST_ROW_PADDING_X,
                    y + (height - leading.height) / 2.0,
                );
                self.pods[pod_index].set_origin(at);
            }
            for side in [SwipeSide::Left, SwipeSide::Right] {
                for action in 0..self.rows[index].actions(side).len() {
                    let pod_index = self.rows[index].action_pod(side, action);
                    let icon = self.pods[pod_index].layout_child(ctx, &action_bc);
                    let Some(slot) = self.action_rect(index, side, action) else {
                        continue;
                    };
                    self.pods[pod_index].set_origin(Point::new(
                        slot.x0 + (slot.width() - icon.width) / 2.0,
                        slot.y0 + (slot.height() - icon.height) / 2.0,
                    ));
                }
            }
            y += height + SWIPEABLE_LIST_ROW_GAP;
        }
        let height = (y - SWIPEABLE_LIST_ROW_GAP).max(0.0);
        bc.constrain(Size::new(self.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let moving = self.advance(ctx.frame_time(), reduce_motion);
        let origin = ctx.origin();

        for index in 0..self.rows.len() {
            self.paint_row(ctx, scene, &colors, index, origin);
        }
        // The travel is a paint-only transform inside an already-measured row,
        // so a bare frame request covers it.
        if moving {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.pods {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some((index, action)) = self.hit(p.position) else {
                    return EventResult::Ignored;
                };
                if self.rows[index].disabled {
                    return EventResult::Ignored;
                }
                self.armed = Some(Armed {
                    id: self.rows[index].id.clone(),
                    action,
                    start: p.position,
                    start_x: self.rows[index].x(),
                    dragging: false,
                });
                // `onDragStart`: a gesture on one row closes whatever other row
                // was open, so at most one is ever revealed.
                if let Some(value) = self.value.clone()
                    && value.id != self.rows[index].id
                {
                    self.request_value(ctx, None);
                }
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(armed) = self.armed.clone() else {
                    let over = self
                        .hit(p.position)
                        .and_then(|(index, action)| action.map(|(side, slot)| (index, side, slot)));
                    if self.hit(p.position).is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                };
                let Some(index) = self.row_of(&armed.id) else {
                    return EventResult::Handled;
                };
                // A press that armed an action never becomes a drag: the button
                // is the gesture.
                if armed.action.is_none() {
                    let raw = armed.start_x + (p.position.x - armed.start.x);
                    let travel = self.constrain(index, raw);
                    self.rows[index].drag = Some(travel);
                    if let Some(live) = self.armed.as_mut() {
                        live.dragging = true;
                    }
                    ctx.request_redraw();
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                let Some(index) = self.row_of(&armed.id) else {
                    return EventResult::Handled;
                };
                if let Some((side, slot)) = armed.action {
                    // A release must land back on the armed action's own rect.
                    let re_hit = self
                        .action_rect(index, side, slot)
                        .is_some_and(|rect| rect.contains(p.position));
                    if re_hit {
                        self.fire_action(ctx, index, side, slot);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                self.settle_release(ctx, index);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if let Some(index) = self.row_of(&armed.id)
                    && armed.action.is_none()
                {
                    // A cancelled swipe returns to whatever the owner confirmed.
                    let displayed = self.rows[index].x();
                    let target = self.target_x(index);
                    self.rows[index].drag = None;
                    self.rows[index].travel =
                        Lane::at_rest(Ramp::spring(SWIPEABLE_LIST_ROW_SETTLE), displayed);
                    self.rows[index].travel.retarget(target);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::List,
            |_| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    let label = row_label(row);
                    let disabled = row.disabled;
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(label.as_str());
                        if disabled {
                            node.set_disabled();
                        }
                    });
                    // Only the revealed rail is published — the input-parity
                    // carve-out, and upstream's own `inert={!openSide}`.
                    let Some(side) = self.open_side(index) else {
                        continue;
                    };
                    for action in row.actions(side) {
                        let name = action.label.clone();
                        let action_disabled = action.disabled || disabled;
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(name.as_str());
                            if action_disabled {
                                node.set_disabled();
                            } else {
                                node.add_action(Action::Click);
                            }
                        });
                    }
                }
            },
        );
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for pod in &self.pods {
            visitor(pod);
        }
    }
}

/// A row's accessible name: its title, its description and its meta note, in
/// reading order.
fn row_label(row: &RowState) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for run in [&row.title, &row.description, &row.meta]
        .into_iter()
        .flatten()
    {
        parts.push(run.content());
    }
    parts.join(". ")
}

impl SwipeableListWidget {
    /// Apply upstream's release rules to row `index` and report the result.
    fn settle_release(&mut self, ctx: &mut EventCtx, index: usize) {
        let latest = self.rows[index].x();
        let side = self.release_side(index, latest);
        let next = side.map(|side| SwipeableListValue {
            id: self.rows[index].id.clone(),
            side,
        });
        // The spring starts from what is on screen, whatever the owner does
        // with the report — a released row must never jump.
        let target = match side {
            Some(SwipeSide::Left) => self.rail_width(index, SwipeSide::Left),
            Some(SwipeSide::Right) => -self.rail_width(index, SwipeSide::Right),
            None => 0.0,
        };
        self.rows[index].drag = None;
        self.rows[index].travel = Lane::at_rest(Ramp::spring(SWIPEABLE_LIST_ROW_SETTLE), latest);
        self.rows[index].travel.retarget(target);
        self.request_value(ctx, next);
    }

    /// Report action `slot` on row `index`'s `side` rail, then close the row if
    /// `closeOnAction`.
    fn fire_action(&mut self, ctx: &mut EventCtx, index: usize, side: SwipeSide, slot: usize) {
        let Some(action) = self.rows[index].actions(side).get(slot) else {
            return;
        };
        if action.disabled {
            return;
        }
        let payload = SwipeActionEvent {
            item_id: self.rows[index].id.clone(),
            action_id: action.id.clone(),
            side,
        };
        if let Some(on_action) = self.on_action.as_mut() {
            on_action(ctx, payload);
        }
        if self.close_on_action {
            let displayed = self.rows[index].x();
            self.rows[index].drag = None;
            self.rows[index].travel =
                Lane::at_rest(Ramp::spring(SWIPEABLE_LIST_ROW_SETTLE), displayed);
            self.rows[index].travel.retarget(0.0);
            self.request_value(ctx, None);
        }
    }

    /// Paint one row: its well, the rail behind it, and the displaced surface.
    fn paint_row(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        colors: &SwipeableListColors,
        index: usize,
        origin: Point,
    ) {
        let rect = self.rows[index].rect(self.width);
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let at = origin + rect.origin().to_vec2();
        let radius = style::resolve_radius(SWIPEABLE_LIST_RADIUS, rect.width(), rect.height());
        let dim = self.rows[index].disabled;
        if dim {
            scene.push_layer(at, rect.size(), SWIPEABLE_LIST_DISABLED_OPACITY);
        }
        // `overflow-hidden`: the surface slides inside the well, never past it.
        scene.push_clip_rounded(at, rect.size(), radius);
        scene.fill_rounded_rect(at, rect.size(), radius, colors.well);

        // The rail, under the surface. Both sides are drawn — the surface is
        // what hides the one that is not revealed.
        for side in [SwipeSide::Left, SwipeSide::Right] {
            for slot in 0..self.rows[index].actions(side).len() {
                let Some(cell) = self.action_rect(index, side, slot) else {
                    continue;
                };
                let entry = &self.rows[index].actions(side)[slot];
                let tone = colors.tone(entry.tone);
                let hovered = self.hovered == Some((index, side, slot));
                let pressed = self
                    .armed
                    .as_ref()
                    .is_some_and(|a| a.id == self.rows[index].id && a.action == Some((side, slot)));
                let scale = if pressed {
                    SWIPEABLE_LIST_ACTION_PRESS_SCALE
                } else {
                    1.0
                };
                let centre = Point::new(
                    origin.x + cell.x0 + cell.width() / 2.0,
                    origin.y + cell.y0 + cell.height() / 2.0,
                );
                if hovered || pressed {
                    let box_size = SWIPEABLE_LIST_ACTION_BOX * scale;
                    scene.fill_rounded_rect(
                        Point::new(centre.x - box_size / 2.0, centre.y - box_size / 2.0),
                        Size::new(box_size, box_size),
                        box_size / 2.0,
                        colors.disc,
                    );
                }
                let pod_index = self.rows[index].action_pod(side, slot);
                let faded = entry.disabled;
                if faded {
                    scene.push_layer(at, rect.size(), style::DISABLED_OPACITY);
                }
                scene.push_transform(
                    Affine::translate(centre.to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-centre.to_vec2()),
                );
                self.pods[pod_index].paint_child(ctx, scene);
                scene.pop_transform();
                if faded {
                    scene.pop_layer();
                }
                let _ = tone;
            }
        }

        // The surface, displaced by the row's travel.
        let travel = self.rows[index].x();
        scene.push_transform(Affine::translate((travel, 0.0)));
        scene.fill_rounded_rect(at, rect.size(), radius, colors.surface);
        crate::press::stroke_outline(scene, at, rect.size(), radius, colors.border);
        self.paint_row_content(ctx, scene, colors, index, at, rect.size());
        scene.pop_transform();

        scene.pop_clip();
        if dim {
            scene.pop_layer();
        }
    }

    /// Paint a row surface's leading slot, text column and meta note.
    fn paint_row_content(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        colors: &SwipeableListColors,
        index: usize,
        at: Point,
        size: Size,
    ) {
        if let Some(pod_index) = self.rows[index].leading_pod() {
            self.pods[pod_index].paint_child(ctx, scene);
        }
        let row = &self.rows[index];
        let leading_width = row
            .leading_pod()
            .map(|pod_index| self.pods[pod_index].size().width)
            .filter(|w| *w > 0.0)
            .map_or(0.0, |w| w + SWIPEABLE_LIST_CONTENT_GAP);
        let text_x = at.x + SWIPEABLE_LIST_ROW_PADDING_X + leading_width;
        let mut right = at.x + size.width - SWIPEABLE_LIST_ROW_PADDING_X;

        if let Some(meta) = &row.meta {
            let meta_size = meta.size();
            meta.paint(
                Point::new(
                    (right - meta_size.width).max(text_x),
                    at.y + (size.height - meta_size.height) / 2.0,
                ),
                colors.dim_ink,
                scene,
            );
            right -= meta_size.width + SWIPEABLE_LIST_CONTENT_GAP;
        }

        let title_h = row.title.as_ref().map_or(0.0, |r| r.size().height);
        let description_h = row.description.as_ref().map(|r| r.size().height);
        let block = title_h + description_h.map_or(0.0, |h| SWIPEABLE_LIST_TITLE_GAP + h);
        let mut y = at.y + (size.height - block) / 2.0;
        // The text column is clipped to the room the meta note left it — the
        // catalog's single-line stand-in for `truncate`.
        scene.push_clip(
            Point::new(text_x, at.y),
            Size::new((right - text_x).max(0.0), size.height),
        );
        if let Some(title) = &row.title {
            title.paint(Point::new(text_x, y), colors.ink, scene);
            y += title_h + SWIPEABLE_LIST_TITLE_GAP;
        }
        if let Some(description) = &row.description {
            description.paint(Point::new(text_x, y), colors.dim_ink, scene);
        }
        scene.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, Brush, PointerButton, PointerEvent, SemanticsUpdate};
    use frust::text;
    use std::any::Any;

    /// Records what the widget paints.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rounded_clips: Vec<(Point, Size, f64)>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_clip(&mut self, _o: Point, _s: Size) {}
        fn push_clip_rounded(&mut self, o: Point, s: Size, r: f64) {
            self.rounded_clips.push((o, s, r));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct App {
        value: Option<Option<SwipeableListValue>>,
        value_calls: u32,
        actions: Vec<SwipeActionEvent>,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<SwipeableListItem<App>> {
        vec![
            swipeable_row("inbox")
                .title("Design review")
                .description("Three files attached")
                .meta("2m")
                .leading(text("D"))
                .left_actions(vec![
                    swipe_action("pin", "Pin", text("P")).tone(SwipeActionTone::Primary),
                ])
                .right_actions(vec![
                    swipe_action("archive", "Archive", text("A")),
                    swipe_action("delete", "Delete", text("X")).tone(SwipeActionTone::Danger),
                ]),
            swipeable_row("release")
                .title("Release notes")
                .right_actions(vec![
                    swipe_action("snooze", "Snooze", text("S")).tone(SwipeActionTone::Warning),
                ]),
            swipeable_row("locked")
                .title("Locked row")
                .disabled(true)
                .right_actions(vec![swipe_action("nope", "Nope", text("N")).disabled(true)]),
        ]
    }

    fn view(value: Option<SwipeableListValue>) -> SwipeableListView<App> {
        swipeable_list::<App, _>(
            items(),
            value,
            |s: &mut App, next: Option<SwipeableListValue>| {
                s.value = Some(next);
                s.value_calls += 1;
            },
        )
        .on_action(|s: &mut App, event: SwipeActionEvent| s.actions.push(event))
    }

    fn build_from(v: &SwipeableListView<App>) -> SwipeableListWidget {
        let mut counter = 0u64;
        View::<App>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut SwipeableListWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 800.0)),
        )
    }

    fn laid_out(value: Option<SwipeableListValue>) -> (SwipeableListWidget, Size) {
        let mut w = build_from(&view(value));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(w: &mut SwipeableListWidget, size: Size, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn settle(w: &mut SwipeableListWidget, size: Size, from_ms: f64) {
        for step in 0..20 {
            paint_at(w, size, from_ms + step as f64 * 200.0);
        }
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut SwipeableListWidget, size: Size, event: &InputEvent, state: &mut App) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// Drag row `index` by `dx` and release, returning the reported value.
    fn swipe(w: &mut SwipeableListWidget, size: Size, index: usize, dx: f64) -> App {
        let mut state = App::default();
        let start = Point::new(
            size.width / 2.0,
            w.rows[index].top + w.rows[index].height / 2.0,
        );
        let end = Point::new(start.x + dx, start.y);
        dispatch(w, size, &pointer(PointerPhase::Down, start), &mut state);
        dispatch(w, size, &pointer(PointerPhase::Move, end), &mut state);
        dispatch(w, size, &pointer(PointerPhase::Up, end), &mut state);
        state
    }

    /// Rows stack in declaration order with the list gap between them, each at
    /// least the 72px floor.
    #[test]
    fn rows_stack_in_order_with_the_list_gap() {
        let (w, size) = laid_out(None);
        assert_eq!(w.rows.len(), 3);
        assert_eq!(w.rows[0].top, 0.0);
        for pair in w.rows.windows(2) {
            assert_eq!(
                pair[1].top,
                pair[0].top + pair[0].height + SWIPEABLE_LIST_ROW_GAP
            );
        }
        for row in &w.rows {
            assert!(row.height >= SWIPEABLE_LIST_ROW_MIN_HEIGHT);
        }
        let last = w.rows.last().unwrap();
        assert_eq!(size.height, last.top + last.height);
        assert_eq!(size.width, 400.0, "the list is `w-full`");
    }

    /// A rail is `actions * actionWidth` wide, and an open row rests exactly on
    /// it — `+leftWidth` right, `−rightWidth` left.
    #[test]
    fn an_open_row_rests_on_its_rail_width() {
        let (mut w, size) = laid_out(Some(SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        }));
        settle(&mut w, size, 0.0);
        assert_eq!(
            w.rail_width(0, SwipeSide::Right),
            2.0 * SWIPEABLE_LIST_ACTION_WIDTH
        );
        assert_eq!(
            w.rail_width(0, SwipeSide::Left),
            SWIPEABLE_LIST_ACTION_WIDTH
        );
        assert!((w.rows[0].x() + 2.0 * SWIPEABLE_LIST_ACTION_WIDTH).abs() < 0.01);

        let (mut left, size) = laid_out(Some(SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Left,
        }));
        settle(&mut left, size, 0.0);
        assert!((left.rows[0].x() - SWIPEABLE_LIST_ACTION_WIDTH).abs() < 0.01);
        assert_eq!(left.rows[1].x(), 0.0, "the other rows stay closed");
    }

    /// The open threshold is upstream's `max(revealThreshold, sideWidth *
    /// 0.46)`: one pixel under it closes, one over it opens.
    #[test]
    fn a_closed_row_opens_only_past_the_reveal_threshold() {
        let (w, _) = laid_out(None);
        let right_width = w.rail_width(0, SwipeSide::Right);
        let threshold =
            SWIPEABLE_LIST_REVEAL_THRESHOLD.max(right_width * SWIPEABLE_LIST_OPEN_RATIO);
        assert_eq!(threshold, right_width * SWIPEABLE_LIST_OPEN_RATIO);

        let (mut under, size) = laid_out(None);
        let state = swipe(&mut under, size, 0, -(threshold - 1.0));
        assert_eq!(state.value, None, "a short drag reports nothing new");

        let (mut over, size) = laid_out(None);
        let state = swipe(&mut over, size, 0, -(threshold + 1.0));
        assert_eq!(
            state.value,
            Some(Some(SwipeableListValue {
                id: "inbox".into(),
                side: SwipeSide::Right
            }))
        );
    }

    /// The `revealThreshold` floor really is a floor: a one-action rail whose
    /// 46% is under 34px still needs the full 34.
    #[test]
    fn the_reveal_threshold_floors_a_narrow_rail() {
        let (w, _) = laid_out(None);
        let narrow = w.rail_width(1, SwipeSide::Right);
        assert_eq!(narrow, SWIPEABLE_LIST_ACTION_WIDTH);
        assert!(
            narrow * SWIPEABLE_LIST_OPEN_RATIO < SWIPEABLE_LIST_REVEAL_THRESHOLD,
            "46% of one slot is under the floor, which is what makes it bind"
        );

        let (mut under, size) = laid_out(None);
        let state = swipe(
            &mut under,
            size,
            1,
            -(SWIPEABLE_LIST_REVEAL_THRESHOLD - 1.0),
        );
        assert_eq!(state.value, None);

        let (mut over, size) = laid_out(None);
        let state = swipe(&mut over, size, 1, -(SWIPEABLE_LIST_REVEAL_THRESHOLD + 1.0));
        assert_eq!(
            state.value,
            Some(Some(SwipeableListValue {
                id: "release".into(),
                side: SwipeSide::Right
            }))
        );
    }

    /// An open row stays open until it is dragged back under 72% of its rail —
    /// upstream's `CLOSE_DISTANCE_RATIO`.
    #[test]
    fn an_open_row_closes_only_under_the_close_ratio() {
        let open = SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        };
        let width = 2.0 * SWIPEABLE_LIST_ACTION_WIDTH;

        // Back a little: still over 72%, stays open (and reports nothing new).
        let (mut held, size) = laid_out(Some(open.clone()));
        settle(&mut held, size, 0.0);
        let state = swipe(&mut held, size, 0, width * 0.2);
        assert_eq!(state.value_calls, 0, "still open, so nothing changed");

        // Back past 72%: closes.
        let (mut closed, size) = laid_out(Some(open.clone()));
        settle(&mut closed, size, 0.0);
        let state = swipe(&mut closed, size, 0, width * 0.4);
        assert_eq!(state.value, Some(None));
    }

    /// The release rules are pure and match upstream's decision tree exactly at
    /// the boundaries, on both sides.
    #[test]
    fn the_release_rules_match_the_upstream_decision_tree() {
        let (w, _) = laid_out(None);
        let right = w.rail_width(0, SwipeSide::Right);
        let left = w.rail_width(0, SwipeSide::Left);
        let right_open = SWIPEABLE_LIST_REVEAL_THRESHOLD.max(right * SWIPEABLE_LIST_OPEN_RATIO);
        let left_open = SWIPEABLE_LIST_REVEAL_THRESHOLD.max(left * SWIPEABLE_LIST_OPEN_RATIO);

        assert_eq!(w.release_side(0, 0.0), None);
        assert_eq!(
            w.release_side(0, -right_open),
            None,
            "exactly at it is not past it"
        );
        assert_eq!(w.release_side(0, -right_open - 0.1), Some(SwipeSide::Right));
        assert_eq!(w.release_side(0, left_open + 0.1), Some(SwipeSide::Left));

        // A row with no left rail never opens left, however far it is dragged.
        assert_eq!(w.release_side(1, 500.0), None);

        let (open, _) = laid_out(Some(SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        }));
        assert_eq!(
            open.release_side(0, -right * SWIPEABLE_LIST_CLOSE_RATIO),
            Some(SwipeSide::Right)
        );
        assert_eq!(
            open.release_side(0, -right * SWIPEABLE_LIST_CLOSE_RATIO + 0.1),
            None
        );
    }

    /// The drag is bounded by the rail, with only `dragElastic` of the excess
    /// getting through — there is no full-swipe past it (the module docs'
    /// premise correction).
    #[test]
    fn the_drag_is_bounded_by_the_rail_with_a_trace_of_elastic() {
        let (w, _) = laid_out(None);
        let right = w.rail_width(0, SwipeSide::Right);
        assert_eq!(w.constrain(0, -right / 2.0), -right / 2.0, "inside is free");
        let over = w.constrain(0, -right - 100.0);
        assert!(
            (over + right + 100.0 * SWIPEABLE_LIST_DRAG_ELASTIC).abs() < 1e-9,
            "100px past the rail moves the row 4px: {over}"
        );
        assert!(
            over > -right - 5.0,
            "the row never travels off its own well"
        );
    }

    /// A live drag actually moves the row, and the release springs it home
    /// rather than snapping.
    #[test]
    fn a_release_springs_home_from_where_the_drag_left_it() {
        let (mut w, size) = laid_out(None);
        let mut state = App::default();
        let start = Point::new(200.0, w.rows[0].height / 2.0);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, start),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(start.x - 20.0, start.y)),
            &mut state,
        );
        assert_eq!(w.rows[0].x(), -20.0, "the row follows the pointer");

        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(start.x - 20.0, start.y)),
            &mut state,
        );
        assert_eq!(state.value, None, "20px is under the threshold");
        assert_eq!(
            w.rows[0].x(),
            -20.0,
            "the spring starts where the drag ended"
        );
        let (_, needs_frame) = paint_at(&mut w, size, 0.0);
        assert!(needs_frame, "a settling row owes frames");
        settle(&mut w, size, 100.0);
        assert_eq!(w.rows[0].x(), 0.0, "and lands exactly closed");
    }

    /// A cancelled swipe returns to whatever the owner confirmed, reporting
    /// nothing.
    #[test]
    fn a_cancelled_swipe_returns_without_reporting() {
        let (mut w, size) = laid_out(None);
        let mut state = App::default();
        let start = Point::new(200.0, w.rows[0].height / 2.0);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, start),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(start.x - 90.0, start.y)),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Cancel, Point::new(start.x - 90.0, start.y)),
            &mut state,
        );
        assert_eq!(state.value_calls, 0, "a cancelled gesture reports nothing");
        settle(&mut w, size, 0.0);
        assert_eq!(w.rows[0].x(), 0.0);
    }

    /// Starting a gesture on another row closes whatever was open —
    /// upstream's `onDragStart`, and the one-row-at-a-time invariant.
    #[test]
    fn a_gesture_on_another_row_closes_the_open_one() {
        let (mut w, size) = laid_out(Some(SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        }));
        settle(&mut w, size, 0.0);
        let mut state = App::default();
        let start = Point::new(200.0, w.rows[1].top + 10.0);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, start),
            &mut state,
        );
        assert_eq!(state.value, Some(None), "the open row was asked to close");
    }

    /// A revealed action is pressable, reports its whole payload, and closes the
    /// row when `closeOnAction` is set.
    #[test]
    fn a_revealed_action_reports_itself_and_closes_the_row() {
        let open = SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        };
        let (mut w, size) = laid_out(Some(open.clone()));
        settle(&mut w, size, 0.0);
        let mut state = App::default();
        let cell = w.action_rect(0, SwipeSide::Right, 1).unwrap();
        let at = cell.center();
        assert_eq!(w.hit(at), Some((0, Some((SwipeSide::Right, 1)))));

        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(
            state.actions,
            vec![SwipeActionEvent {
                item_id: "inbox".into(),
                action_id: "delete".into(),
                side: SwipeSide::Right,
            }]
        );
        assert_eq!(state.value, Some(None), "`closeOnAction` closes the row");

        // ...and with `closeOnAction` off it stays open.
        let mut kept = build_from(&view(Some(open)).close_on_action(false));
        let size = layout(&mut kept);
        settle(&mut kept, size, 0.0);
        let mut state = App::default();
        dispatch(
            &mut kept,
            size,
            &pointer(PointerPhase::Down, at),
            &mut state,
        );
        dispatch(&mut kept, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.actions.len(), 1);
        assert_eq!(state.value_calls, 0);
    }

    /// A closed row's rail is not hit tested at all, and a release off the armed
    /// action fires nothing.
    #[test]
    fn a_closed_rail_is_not_hit_tested_and_a_miss_fires_nothing() {
        let (mut closed, size) = laid_out(None);
        let cell = closed.action_rect(0, SwipeSide::Right, 1).unwrap();
        assert_eq!(
            closed.hit(cell.center()),
            Some((0, None)),
            "a closed row's rail is covered by its own surface"
        );

        let (mut open, size2) = laid_out(Some(SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        }));
        settle(&mut open, size2, 0.0);
        let mut state = App::default();
        let at = open.action_rect(0, SwipeSide::Right, 1).unwrap().center();
        dispatch(
            &mut open,
            size2,
            &pointer(PointerPhase::Down, at),
            &mut state,
        );
        dispatch(
            &mut open,
            size2,
            &pointer(PointerPhase::Up, Point::new(10.0, at.y)),
            &mut state,
        );
        assert!(
            state.actions.is_empty(),
            "a release off the action fires nothing"
        );
        let _ = (&mut closed, size);
    }

    /// A disabled row never drags and a disabled action never fires.
    #[test]
    fn a_disabled_row_never_drags_and_a_disabled_action_never_fires() {
        let (mut w, size) = laid_out(None);
        let state = swipe(&mut w, size, 2, -200.0);
        assert_eq!(state.value_calls, 0, "the disabled row is undraggable");
        assert_eq!(w.rows[2].x(), 0.0);

        let (mut open, size) = laid_out(Some(SwipeableListValue {
            id: "locked".into(),
            side: SwipeSide::Right,
        }));
        settle(&mut open, size, 0.0);
        let mut state = App::default();
        let at = open.action_rect(2, SwipeSide::Right, 0).unwrap().center();
        dispatch(
            &mut open,
            size,
            &pointer(PointerPhase::Down, at),
            &mut state,
        );
        dispatch(&mut open, size, &pointer(PointerPhase::Up, at), &mut state);
        assert!(state.actions.is_empty(), "a disabled action never fires");
    }

    /// The row paints its well, its clipped surface and its displacement, and a
    /// disabled row is dimmed as a whole.
    #[test]
    fn the_row_paints_its_well_surface_and_travel() {
        let (mut w, size) = laid_out(Some(SwipeableListValue {
            id: "inbox".into(),
            side: SwipeSide::Right,
        }));
        settle(&mut w, size, 0.0);
        let (rec, _) = paint_at(&mut w, size, 5_000.0);
        let p = crate::BEUI_LIGHT;
        assert!(
            rec.rrects.iter().any(|(_, _, _, c)| *c == p.muted),
            "the well"
        );
        assert!(
            rec.rrects.iter().any(|(_, _, _, c)| *c == p.card),
            "the surface"
        );
        assert_eq!(rec.rounded_clips.len(), 3, "one rounded clip per row");
        assert!(
            rec.transforms.iter().any(|t| {
                let c = t.as_coeffs();
                (c[4] + 2.0 * SWIPEABLE_LIST_ACTION_WIDTH).abs() < 0.01
            }),
            "the open row's surface is translated onto its rail: {:?}",
            rec.transforms
        );
        assert!(
            rec.layers.contains(&SWIPEABLE_LIST_DISABLED_OPACITY),
            "the disabled row is dimmed as a whole"
        );
        assert!(!rec.inks.is_empty(), "the row text is painted");
    }

    /// `reduce_motion` collapses the release spring: the row is on its target
    /// on the first frame and owes no frame.
    #[test]
    fn reduced_motion_lands_the_release_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(None);
        let mut state = App::default();
        let start = Point::new(200.0, w.rows[0].height / 2.0);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, start),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(start.x - 20.0, start.y)),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(start.x - 20.0, start.y)),
            &mut state,
        );

        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(!ctx.needs_frame(), "a collapsed spring owes no frame");
        assert_eq!(w.rows[0].x(), 0.0);
    }

    /// Every tone resolves to a token, and the two Tailwind palette entries
    /// upstream hardcodes fold onto the catalog's authored `--success` and
    /// `--warning`.
    #[test]
    fn every_tone_resolves_to_a_token() {
        let colors = resolve_colors(None);
        let tokens = BeuiTokens::beui();
        let p = crate::BEUI_LIGHT;
        assert_eq!(colors.tone(SwipeActionTone::Neutral), p.muted_foreground);
        assert_eq!(colors.tone(SwipeActionTone::Primary), p.foreground);
        assert_eq!(colors.tone(SwipeActionTone::Success), tokens.success);
        assert_eq!(colors.tone(SwipeActionTone::Warning), tokens.warning);
        assert_eq!(colors.tone(SwipeActionTone::Danger), p.destructive);
    }

    /// The list publishes one item per row and, for the open row only, one
    /// button per revealed action — upstream's `inert={!openSide}`.
    #[test]
    fn semantics_publish_the_rows_and_only_the_revealed_rail() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = App::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut App| {
            view(Some(SwipeableListValue {
                id: "inbox".into(),
                side: SwipeSide::Right,
            }))
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(400.0, 800.0), &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let rows: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::ListItem)
            .collect();
        assert_eq!(rows.len(), 3, "one node per row");
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(
            buttons.len(),
            2,
            "only the open row's two actions are published"
        );
        assert!(
            rows.iter()
                .any(|(_, n)| n.label() == Some("Design review. Three files attached. 2m")),
            "a row is named by its whole content"
        );
    }

    // ---- Typeface: a row's title, description and meta follow the theme ---

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(480.0, 400.0);

    /// A full row and a title-only one, at rest.
    fn probe_view(_: &mut ()) -> SwipeableListView<()> {
        swipeable_list::<(), _>(
            vec![
                swipeable_row("inbox")
                    .title("Design review")
                    .description("Three files attached")
                    .meta("2m"),
                swipeable_row("release").title("Release notes"),
            ],
            None,
            |_: &mut (), _| {},
        )
    }

    #[test]
    fn row_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the rows' text", probe_view, PROBE_WINDOW);
        let runs = Probe::new(probe_view, PROBE_WINDOW, crate::theme()).frame();
        assert_eq!(runs.len(), 4, "two titles, a description and a meta stamp");
    }

    #[test]
    fn row_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the rows' text", probe_view, PROBE_WINDOW);
    }
}
