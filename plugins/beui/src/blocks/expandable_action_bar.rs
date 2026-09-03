//! Ports beUI's `expandable-action-bar` block —
//! `components/motion/expandable-action-bar.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `expandable-action-bar`: *"Compact icon actions that expand into labeled
//! controls on hover or focus with shared layout motion."*
//!
//! | upstream | here |
//! |---|---|
//! | track `rounded-full border border-border bg-card/90 shadow-2xl` | the shared panel chrome at [`style::RADIUS_CONTROL`] |
//! | `min-h-11 gap-1.5 p-1.5 text-sm` / `min-h-9 gap-1 p-1 text-xs` | [`ActionBarSize::Md`] / [`ActionBarSize::Sm`] |
//! | item `h-8 min-w-8 px-2 rounded-full` / `h-7 min-w-7 px-1.5` | [`ActionBarSize::item_height`], [`ActionBarSize::item_padding_x`] |
//! | icon `h-4 w-4` / `h-3.5 w-3.5` | [`ActionBarSize::icon_size`] |
//! | `ITEM_TRANSITION` `{stiffness 460, damping 34, mass 0.62}` | [`ACTION_BAR_ITEM`] |
//! | `LABEL_TRANSITION` `{stiffness 380, damping 32, mass 0.7}` | [`ACTION_BAR_LABEL`] |
//! | label `width 0 → auto`, `marginLeft 0 → 8`, `x −4 → 0`, `opacity 0 → 1` | the per-item reveal ([`ACTION_BAR_LABEL_GAP`], [`ACTION_BAR_LABEL_SHIFT`]) |
//! | highlight `layoutId` `bg-primary/[0.07] rounded-full` | [`ACTION_BAR_HIGHLIGHT_ALPHA`] |
//! | `collapseDelay = 90` | [`ACTION_BAR_COLLAPSE_DELAY`] |
//! | `disabled:opacity-40` | [`ACTION_BAR_DISABLED_OPACITY`] |
//! | shortcut `text-[10px] text-muted-foreground`, `marginLeft 4` | [`ActionBarItem::shortcut`], [`ACTION_BAR_SHORTCUT_GAP`] |
//!
//! # The expansion is a layout animation, so it is timed at paint and applied
//! at layout
//!
//! Each item's width travels between its collapsed square and its labelled
//! width, which resizes the whole bar — a *layout* animation, and the framework
//! gives `layout` no clock. The catalog's answer, the one
//! [`crate::components::combobox`] documents for its own animated panel height:
//! the paint pass advances the run, records each item's reveal, and asks for a
//! relayout while any of them is moving; the next `layout` reads those recorded
//! values. The cost is the documented one-frame lag, not a second clock.
//!
//! # The stagger is an addition
//!
//! Upstream reveals every label together on one `LABEL_TRANSITION`. The rows
//! here arrive [`ACTION_BAR_STAGGER`] apart, leading from the first item, and
//! the collapse unwinds on [`crate::motion::stagger::EXIT_DELAY_FACTOR`]'s
//! halved delays — the catalog's own enter/exit asymmetry. `reduce_motion`
//! collapses it back to upstream's single beat.
//!
//! # Hover, focus, and the collapse delay
//!
//! Hover is resolved the way `docs/CODE_STANDARDS.md`'s three-part rule
//! requires: claimed from the uncaptured `Move` arm, latched into the widget's
//! own flag, and self-corrected from `PaintCtx::is_hovered` every paint. The
//! `collapseDelay` upstream implements with `window.setTimeout` is a frame-clock
//! hysteresis here — the paint that first reads "no longer hovered" records its
//! own frame time and keeps asking for frames until
//! [`ACTION_BAR_COLLAPSE_DELAY`] has passed, which is the same 90ms without a
//! timer (`frust-core`/`frust-widgets` read no wall clock).
//!
//! Because the collapse is *noticed* at paint time, where no callback can be
//! fired, [`ExpandableActionBarView::on_expanded_change`] is reported one event
//! pass late, through the same drainable latch
//! [`crate::motion::Presence::take_exited`] uses for the same reason.
//!
//! # Degradations against the web original
//!
//! - **No touch two-tap.** Upstream's first tap expands and the second acts,
//!   keyed off `gesture.pointerType`; `PointerEvent` here carries no pointer
//!   type (the gap [`crate::components::context_menu`] records for its own
//!   long-press), so every press acts and the labels are reached by hover or
//!   focus. The bar stays usable by touch — it is icons plus an accessible name
//!   — it simply does not reveal its labels on the way.
//! - **No outside-tap dismissal.** It exists upstream only to close a
//!   *tap*-expanded bar, which the previous point removes.
//! - **The keyboard moves a bar-local highlight, not focus.** Upstream tabs
//!   between item buttons; the whole bar is one widget here, so the arrows move
//!   the highlight and Enter/Space activates it — the same call
//!   [`crate::components::context_menu`] makes, with the same visible result.
//! - **No badges.** `ExpandableActionBarItem.badge` is an arbitrary node; the
//!   icon slot is the one child protocol this component takes (the shape
//!   [`crate::components::dock`] established), so a badge is left to the caller's
//!   own icon view.
//! - **No `renderItem` escape hatch**, which is a JSX-only seam.
//! - **The track is opaque.** `bg-card/90 backdrop-blur-xl` has no blur
//!   primitive; the catalog's panel chrome stands in, as it does for every
//!   surface here.
//! - **The rail does not scroll.** Upstream's track is `overflow-x-auto` so a
//!   labelled bar wider than its box can still be reached; here the bar clamps
//!   to its constraints and the trailing items clip — a scrollable rail needs a
//!   [`frust::scroll_view`], whose infinite cross-axis constraint this
//!   layout-animated width cannot resolve against.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerEvent,
    PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2, View, Widget, any, build_child,
    erase_callback_arg, rebuild_children, text::TextStyle, visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::components::popover::{PanelChrome, lerp, paint_panel, resolve_panel};
use crate::motion::stagger::StaggerDirection;
use crate::motion::{Ramp, Stagger};
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::LabelRun;

// ---- Metrics ---------------------------------------------------------------

/// The two size rungs upstream ships (`OverflowActionsSize`'s twin,
/// `ExpandableActionBarSize`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionBarSize {
    /// `min-h-9 gap-1 p-1 text-xs`, items `h-7 min-w-7 px-1.5`.
    Sm,
    /// `min-h-11 gap-1.5 p-1.5 text-sm`, items `h-8 min-w-8 px-2`.
    #[default]
    Md,
}

impl ActionBarSize {
    /// `p-1` / `p-1.5` — the track's own padding, in logical px.
    pub const fn padding(self) -> f64 {
        match self {
            ActionBarSize::Sm => 4.0,
            ActionBarSize::Md => 6.0,
        }
    }

    /// `gap-1` / `gap-1.5` — the gap between items, in logical px.
    pub const fn gap(self) -> f64 {
        self.padding()
    }

    /// `h-7` / `h-8` — one item's height, in logical px.
    pub const fn item_height(self) -> f64 {
        match self {
            ActionBarSize::Sm => 28.0,
            ActionBarSize::Md => 32.0,
        }
    }

    /// `min-w-7` / `min-w-8` — one item's collapsed width, in logical px.
    pub const fn item_min_width(self) -> f64 {
        self.item_height()
    }

    /// `px-1.5` / `px-2` — an item's horizontal padding, in logical px.
    pub const fn item_padding_x(self) -> f64 {
        match self {
            ActionBarSize::Sm => 6.0,
            ActionBarSize::Md => 8.0,
        }
    }

    /// `h-3.5` / `h-4` — an icon's box, in logical px.
    pub const fn icon_size(self) -> f64 {
        match self {
            ActionBarSize::Sm => 14.0,
            ActionBarSize::Md => 16.0,
        }
    }

    /// `text-xs` / `text-sm` — a label's size, in logical px.
    pub const fn text_size(self) -> f64 {
        match self {
            ActionBarSize::Sm => style::TEXT_XS,
            ActionBarSize::Md => style::TEXT_SM,
        }
    }

    /// `min-h-9` / `min-h-11` — the track's least height, in logical px.
    pub const fn min_height(self) -> f64 {
        match self {
            ActionBarSize::Sm => 36.0,
            ActionBarSize::Md => 44.0,
        }
    }
}

/// `marginLeft: 8` — the gap an expanded label opens between itself and its
/// icon, in logical px.
pub const ACTION_BAR_LABEL_GAP: f64 = 8.0;

/// `x: -4` — how far a hidden label sits behind its resting position, in
/// logical px.
pub const ACTION_BAR_LABEL_SHIFT: f64 = 4.0;

/// `marginLeft: 4` — the gap before an item's shortcut, in logical px.
pub const ACTION_BAR_SHORTCUT_GAP: f64 = 4.0;

/// `text-[10px]` — a shortcut's size, in logical px.
pub const ACTION_BAR_SHORTCUT_TEXT: f64 = 10.0;

/// `bg-primary/[0.07]` — the highlight pill's alpha over the primary role.
pub const ACTION_BAR_HIGHLIGHT_ALPHA: f32 = 0.07;

/// `disabled:opacity-40` — a disabled item's ink opacity.
pub const ACTION_BAR_DISABLED_OPACITY: f32 = 0.40;

// ---- Motion ----------------------------------------------------------------

/// `ITEM_TRANSITION` — the highlight's travel and the track's own resize.
pub const ACTION_BAR_ITEM: SpringDescription = SpringDescription {
    mass: 0.62,
    stiffness: 460.0,
    damping: 34.0,
};

/// `LABEL_TRANSITION` — one label's own reveal.
pub const ACTION_BAR_LABEL: SpringDescription = SpringDescription {
    mass: 0.7,
    stiffness: 380.0,
    damping: 32.0,
};

/// How far apart consecutive items reveal. An addition — see the [module
/// docs](self).
pub const ACTION_BAR_STAGGER: Duration = Duration::from_millis(28);

/// `collapseDelay = 90` — how long the bar stays open after the pointer leaves.
pub const ACTION_BAR_COLLAPSE_DELAY: Duration = Duration::from_millis(90);

// ---- Items -----------------------------------------------------------------

/// One action on the bar: an icon view, its label, and the flags upstream's
/// `ExpandableActionBarItem` carries (minus the badge the [module docs](self)
/// record).
pub struct ActionBarItem<State: 'static> {
    icon: AnyView<State>,
    label: String,
    shortcut: Option<String>,
    disabled: bool,
    active: bool,
}

/// An action drawing `icon` and labelled `label`.
///
/// The icon is a **view**, the child protocol [`crate::components::dock`]
/// established for the same reason: the catalog ships no icon vocabulary, so the
/// caller supplies the mark and this component supplies the box, the label and
/// the motion.
pub fn action_bar_item<State: 'static, V: View<State>>(
    icon: V,
    label: impl Into<String>,
) -> ActionBarItem<State> {
    ActionBarItem {
        icon: any(icon),
        label: label.into(),
        shortcut: None,
        disabled: false,
        active: false,
    }
}

impl<State: 'static> ActionBarItem<State> {
    /// Set the trailing shortcut hint (`shortcut`).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Make this action unactivatable (`disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mark this action as the current one (`active`), which is where the
    /// highlight rests when nothing is hovered.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

// ---- The component ---------------------------------------------------------

/// A view-held action callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State, usize)>;

/// A view-held expansion callback (erased on build).
type OnExpandedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// What the bar renders from, beyond its items.
#[derive(Clone, Debug, PartialEq)]
struct BarConfig {
    /// The controlled override, when the app owns the expansion.
    expanded: Option<bool>,
    expand_on_hover: bool,
    expand_on_focus: bool,
    size: ActionBarSize,
}

/// A declarative beUI expandable action bar. See [`expandable_action_bar`].
pub struct ExpandableActionBarView<State: 'static> {
    items: Vec<ActionBarItem<State>>,
    config: BarConfig,
    on_action: OnAction<State>,
    on_expanded_change: OnExpandedChange<State>,
}

/// Build an action bar over `items`, compact until hovered or focused.
///
/// `on_action(state, index)` reports an activation with the item's index.
pub fn expandable_action_bar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<ActionBarItem<State>>,
    on_action: F,
) -> ExpandableActionBarView<State> {
    ExpandableActionBarView {
        items,
        config: BarConfig {
            expanded: None,
            expand_on_hover: true,
            expand_on_focus: true,
            size: ActionBarSize::default(),
        },
        on_action: Rc::new(on_action),
        on_expanded_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> ExpandableActionBarView<State> {
    /// Take the expansion over: the bar shows exactly what `expanded` says and
    /// stops resolving hover and focus for itself (upstream's controlled
    /// `expanded` prop).
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.config.expanded = Some(expanded);
        self
    }

    /// Expand while the pointer rests on the bar (default `true`).
    pub fn expand_on_hover(mut self, expand_on_hover: bool) -> Self {
        self.config.expand_on_hover = expand_on_hover;
        self
    }

    /// Expand while the bar holds focus (default `true`).
    pub fn expand_on_focus(mut self, expand_on_focus: bool) -> Self {
        self.config.expand_on_focus = expand_on_focus;
        self
    }

    /// Pick the size rung (default [`ActionBarSize::Md`]).
    pub fn size(mut self, size: ActionBarSize) -> Self {
        self.config.size = size;
        self
    }

    /// Set the expansion callback. Reported **one event pass late** — see the
    /// [module docs](self).
    pub fn on_expanded_change<F: Fn(&mut State, bool) + 'static>(
        mut self,
        on_expanded_change: F,
    ) -> Self {
        self.on_expanded_change = Rc::new(on_expanded_change);
        self
    }
}

/// One laid-out action.
struct Entry {
    label: LabelRun,
    shortcut: Option<LabelRun>,
    disabled: bool,
    active: bool,
    /// The item's box in the bar's own space.
    rect: Rect,
    /// The label's own width once shaped, in logical px.
    label_width: f64,
    /// The shortcut's width once shaped, in logical px.
    shortcut_width: f64,
}

/// The retained widget for an [`ExpandableActionBarView`].
pub struct ExpandableActionBarWidget {
    /// One icon pod per action.
    icons: Vec<ChildPod>,
    entries: Vec<Entry>,
    config: BarConfig,
    /// Each item's reveal, `0.0` collapsed to `1.0` labelled, as of the last
    /// paint — the values the next `layout` sizes from.
    reveals: Vec<f64>,
    /// Whether the bar is currently showing its labels.
    expanded: bool,
    /// The frame the current reveal run started on.
    run_started: Option<FrameTime>,
    /// Which way that run is travelling.
    direction: StaggerDirection,
    /// When the pointer stopped resting on the bar, for the collapse delay.
    unhovered_at: Option<FrameTime>,
    /// The latched hover flag (the three-part hover rule).
    hovered: bool,
    /// The item the highlight sits on.
    highlight: Option<usize>,
    /// The item a primary `Down` armed.
    armed: Option<usize>,
    /// The highlight's travel and opacity.
    pill: Lane,
    pill_from: Rect,
    pill_to: Rect,
    pill_alpha: Lane,
    /// An expansion change noticed at paint, waiting for an event pass to
    /// report it (see the [module docs](self)).
    pending_expanded: Option<bool>,
    on_action: ErasedArgCallback<usize>,
    on_expanded_change: ErasedArgCallback<bool>,
}

impl ExpandableActionBarWidget {
    /// Whether the bar is showing its labels.
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// The item the highlight sits on.
    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// One item's box in the bar's own space.
    pub fn item_rect(&self, index: usize) -> Option<Rect> {
        self.entries.get(index).map(|entry| entry.rect)
    }

    /// One item's reveal as of the last paint: `0.0` collapsed, `1.0` labelled.
    pub fn reveal(&self, index: usize) -> f64 {
        self.reveals.get(index).copied().unwrap_or(0.0)
    }

    /// The width an item takes at `reveal`, in logical px.
    ///
    /// Public because it *is* the component's layout contract: the collapsed
    /// square grows by the label (and its gap, and any shortcut) exactly in
    /// step with the reveal.
    pub fn item_width(&self, index: usize, reveal: f64) -> f64 {
        let size = self.config.size;
        let collapsed = size
            .item_min_width()
            .max(size.icon_size() + size.item_padding_x() * 2.0);
        let Some(entry) = self.entries.get(index) else {
            return collapsed;
        };
        let mut grown = collapsed + ACTION_BAR_LABEL_GAP + entry.label_width;
        if entry.shortcut.is_some() {
            grown += ACTION_BAR_SHORTCUT_GAP + entry.shortcut_width;
        }
        collapsed + (grown - collapsed) * reveal.clamp(0.0, 1.0)
    }

    /// The stagger the reveals run on, in the direction the bar is travelling.
    fn stagger(&self, reduce: bool) -> Stagger {
        let base = Stagger::new(ACTION_BAR_STAGGER, Ramp::spring(ACTION_BAR_LABEL))
            .direction(self.direction);
        if reduce { base.collapsed() } else { base }
    }

    /// Move the highlight onto `next`, springing from wherever it is now.
    fn set_highlight(&mut self, next: Option<usize>) -> bool {
        if self.highlight == next {
            return false;
        }
        let current = self.pill_rect();
        self.highlight = next;
        if let Some(entry) = next.and_then(|i| self.entries.get(i)) {
            self.pill_from = if self.pill_alpha.target() == 0.0 && current.is_zero_area() {
                entry.rect
            } else {
                current
            };
            self.pill_to = entry.rect;
            self.pill = Lane::at_rest(Ramp::spring(ACTION_BAR_ITEM), 0.0);
            self.pill.retarget(1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(ACTION_BAR_ITEM), 1.0);
        } else {
            self.pill_from = current;
            self.pill_to = current;
            self.pill = Lane::at_rest(Ramp::spring(ACTION_BAR_ITEM), 1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(ACTION_BAR_ITEM), 0.0);
        }
        true
    }

    /// The highlight pill's rect right now.
    fn pill_rect(&self) -> Rect {
        if self.pill_from.is_zero_area() && self.pill_to.is_zero_area() {
            return self
                .highlight
                .and_then(|i| self.entries.get(i))
                .map_or(Rect::ZERO, |entry| entry.rect);
        }
        let t = self.pill.value().clamp(0.0, 1.0);
        Rect::new(
            lerp(self.pill_from.x0, self.pill_to.x0, t),
            lerp(self.pill_from.y0, self.pill_to.y0, t),
            lerp(self.pill_from.x1, self.pill_to.x1, t),
            lerp(self.pill_from.y1, self.pill_to.y1, t),
        )
    }

    /// The item the pointer is over, if it can be activated.
    fn item_at(&self, position: Point) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| !entry.disabled && entry.rect.contains(position))
    }

    /// The next enabled item in `step`'s direction, wrapping — the walk
    /// [`crate::components::context_menu`] uses for its own rows.
    fn step_highlight(&self, step: isize) -> Option<usize> {
        let count = self.entries.len();
        if count == 0 {
            return None;
        }
        let start = self
            .highlight
            .map_or(if step > 0 { count - 1 } else { 0 }, |i| i);
        for hop in 1..=count {
            let index = (start as isize + step * hop as isize).rem_euclid(count as isize) as usize;
            if !self.entries[index].disabled {
                return Some(index);
            }
        }
        None
    }

    /// Where the highlight rests with nothing hovered: the active item.
    fn resting_highlight(&self) -> Option<usize> {
        self.entries.iter().position(|entry| entry.active)
    }

    /// Stage a change of expansion, restarting the reveal run in its new
    /// direction.
    fn set_expanded(&mut self, expanded: bool) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        self.direction = if expanded {
            StaggerDirection::Enter
        } else {
            StaggerDirection::Exit
        };
        self.run_started = None;
        self.pending_expanded = Some(expanded);
    }
}

impl<State: 'static> View<State> for ExpandableActionBarView<State> {
    type Element = ExpandableActionBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ExpandableActionBarWidget {
        let expanded = self.config.expanded.unwrap_or(false);
        let entries: Vec<Entry> = self.items.iter().map(Entry::from_item).collect();
        let resting = entries.iter().position(|entry| entry.active);
        ExpandableActionBarWidget {
            icons: self
                .items
                .iter()
                .map(|item| build_child(&item.icon, ctx))
                .collect(),
            reveals: vec![if expanded { 1.0 } else { 0.0 }; entries.len()],
            entries,
            config: self.config.clone(),
            expanded,
            run_started: None,
            // The resting direction has to match the resting state: a collapsed
            // run reads `1 - progress`, which is zero once it settles, while an
            // `Enter` run at rest would reveal everything the moment any time
            // passed.
            direction: if expanded {
                StaggerDirection::Enter
            } else {
                StaggerDirection::Exit
            },
            unhovered_at: None,
            hovered: false,
            highlight: resting,
            armed: None,
            pill: Lane::at_rest(Ramp::spring(ACTION_BAR_ITEM), 1.0),
            pill_from: Rect::ZERO,
            pill_to: Rect::ZERO,
            pill_alpha: Lane::at_rest(
                Ramp::spring(ACTION_BAR_ITEM),
                if resting.is_some() { 1.0 } else { 0.0 },
            ),
            pending_expanded: None,
            on_action: erase_callback_arg(&self.on_action),
            on_expanded_change: erase_callback_arg(&self.on_expanded_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ExpandableActionBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.entries.len() != self.items.len() {
            element.entries = self.items.iter().map(Entry::from_item).collect();
            element.reveals = vec![if element.expanded { 1.0 } else { 0.0 }; element.entries.len()];
            element.highlight = element.resting_highlight();
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, item) in element.entries.iter_mut().zip(&self.items) {
                if entry.sync(item) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        if element.config != self.config {
            if element.config.size != self.config.size {
                flags |= ChangeFlags::LAYOUT;
            }
            element.config = self.config.clone();
            if let Some(expanded) = self.config.expanded {
                element.set_expanded(expanded);
                // A controlled flag is the app's own value coming back down;
                // reporting it to the app again would be an echo.
                element.pending_expanded = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.icons,
            ctx,
            |item: &ActionBarItem<State>| &item.icon,
            |_| None::<ChildKey>,
        );
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_action = erase_callback_arg(&self.on_action);
        element.on_expanded_change = erase_callback_arg(&self.on_expanded_change);
        flags
    }

    fn teardown(&self, element: &mut ExpandableActionBarWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.icons.iter_mut()) {
            frust::authoring::teardown_child(&item.icon, pod, ctx);
        }
    }
}

impl Entry {
    fn from_item<State: 'static>(item: &ActionBarItem<State>) -> Self {
        Entry {
            label: LabelRun::new(item.label.clone()),
            shortcut: item.shortcut.clone().map(LabelRun::new),
            disabled: item.disabled,
            active: item.active,
            rect: Rect::ZERO,
            label_width: 0.0,
            shortcut_width: 0.0,
        }
    }

    /// Adopt `item`, reporting whether anything the layout depends on changed.
    fn sync<State: 'static>(&mut self, item: &ActionBarItem<State>) -> bool {
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
        changed |= self.disabled != item.disabled || self.active != item.active;
        self.disabled = item.disabled;
        self.active = item.active;
        changed
    }
}

/// A label's style at `size`.
fn label_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        ..crate::text::label_style(size)
    }
}

impl Widget for ExpandableActionBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let size = self.config.size;
        let label_style = label_style(theme, size.text_size());
        let shortcut_style = label_style_small(theme);

        for entry in &mut self.entries {
            entry.label_width = entry.label.layout(ctx, &label_style).width;
            entry.shortcut_width = entry
                .shortcut
                .as_mut()
                .map_or(0.0, |run| run.layout(ctx, &shortcut_style).width);
        }

        let height = size
            .min_height()
            .max(size.item_height() + size.padding() * 2.0);
        let icon_bc = BoxConstraints::tight(Size::new(size.icon_size(), size.icon_size()));
        let mut x = size.padding();
        for index in 0..self.entries.len() {
            let width = self.item_width(index, self.reveal(index));
            let top = (height - size.item_height()) / 2.0;
            self.entries[index].rect = Rect::new(x, top, x + width, top + size.item_height());
            if let Some(icon) = self.icons.get_mut(index) {
                icon.layout_child(ctx, &icon_bc);
                icon.set_origin(Point::new(
                    x + size.item_padding_x(),
                    top + (size.item_height() - size.icon_size()) / 2.0,
                ));
            }
            x += width + size.gap();
        }
        let width = if self.entries.is_empty() {
            size.padding() * 2.0
        } else {
            x - size.gap() + size.padding()
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let accent = theme.map_or(crate::BEUI_LIGHT.primary, |t| t.scheme().primary);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();

        // Hover self-correction — authoritative, and the only signal a pointer
        // that simply left the bar produces.
        let hovered = ctx.is_hovered();
        if self.hovered != hovered {
            self.hovered = hovered;
        }
        self.resolve_expansion(ctx, now);

        // The track: a pill of the catalog's own panel chrome.
        let track = Rect::from_origin_size(Point::ORIGIN, ctx.size());
        let radius = style::resolve_radius(style::RADIUS_CONTROL, track.width(), track.height());
        paint_panel(scene, origin, track, radius, chrome);

        // The reveals, on their staggered run — recorded for the next layout.
        let started = *self.run_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let stagger = self.stagger(reduce);
        let count = self.entries.len();
        if !stagger.is_settled(elapsed, count) {
            // A layout-affecting animation asks for a relayout, never a bare
            // repaint (WIDGETS_CODE_STANDARDS' animation rule).
            ctx.request_layout();
        }
        self.reveals = (0..count)
            .map(|index| stagger.revealed(elapsed, index, count))
            .collect();

        // The shared highlight, under the items.
        let travelled = self.pill.advance(now);
        let faded = self.pill_alpha.advance(now);
        if travelled || faded {
            ctx.request_frame();
        }
        let alpha = self.pill_alpha.value().clamp(0.0, 1.0) as f32;
        if alpha > 0.0 {
            let pill = self.pill_rect();
            if pill.width() > 0.0 && pill.height() > 0.0 {
                scene.fill_rounded_rect(
                    origin + pill.origin().to_vec2(),
                    pill.size(),
                    style::resolve_radius(style::RADIUS_CONTROL, pill.width(), pill.height()),
                    style::with_alpha(accent, ACTION_BAR_HIGHLIGHT_ALPHA * alpha),
                );
            }
        }

        for index in 0..count {
            self.paint_item(ctx, scene, chrome, index);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for icon in &mut self.icons {
                icon.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // Drain the expansion the paint pass noticed — the deferral the module
        // docs describe.
        if let Some(expanded) = self.pending_expanded.take() {
            (self.on_expanded_change)(ctx, expanded);
        }
        match event {
            InputEvent::Key(key) => self.handle_key(ctx, key),
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Toolbar,
            |_| {},
            |ctx| {
                for entry in &self.entries {
                    let label = entry.label.content().to_string();
                    let disabled = entry.disabled;
                    let active = entry.active;
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(label.as_str());
                        node.set_selected(active);
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

    visit_children!(icons);
}

/// A shortcut's style (`text-[10px]`).
fn label_style_small(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_small.family.clone()
    });
    TextStyle {
        family,
        ..TextStyle::new(ACTION_BAR_SHORTCUT_TEXT as f32, crate::text::SHAPING_INK)
    }
}

impl ExpandableActionBarWidget {
    /// Resolve whether the bar should be showing its labels this frame, and
    /// stage the run when that changes.
    ///
    /// The controlled override wins outright; otherwise hover and focus decide,
    /// with [`ACTION_BAR_COLLAPSE_DELAY`]'s hysteresis on the way down.
    fn resolve_expansion(&mut self, ctx: &mut PaintCtx, now: FrameTime) {
        if self.config.expanded.is_some() {
            return;
        }
        let holding = (self.config.expand_on_hover && self.hovered)
            || (self.config.expand_on_focus && ctx.has_focus());
        if holding {
            self.unhovered_at = None;
            self.set_expanded(true);
            return;
        }
        if !self.expanded {
            return;
        }
        // `collapseDelay`, on the frame clock rather than a `setTimeout`.
        let since = *self.unhovered_at.get_or_insert(now);
        if now.saturating_sub(since) >= ACTION_BAR_COLLAPSE_DELAY {
            self.unhovered_at = None;
            self.set_expanded(false);
        } else {
            ctx.request_frame();
        }
    }

    /// Paint one item: its icon pod, its label, its shortcut.
    fn paint_item(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        chrome: PanelChrome,
        index: usize,
    ) {
        let origin = ctx.origin();
        let size = self.config.size;
        let reveal = self.reveal(index).clamp(0.0, 1.0);
        let entry = &self.entries[index];
        let highlighted = self.highlight == Some(index);
        let ink = style::disabled_tint(
            if highlighted {
                chrome.ink
            } else {
                chrome.dim_ink
            },
            entry.disabled,
            ACTION_BAR_DISABLED_OPACITY,
        );

        if let Some(icon) = self.icons.get_mut(index) {
            icon.paint_child(ctx, scene);
        }
        if reveal <= 0.0 {
            return;
        }
        let entry = &self.entries[index];
        // `x: -4 → 0` alongside the fade, so the label slides into place rather
        // than appearing where it lands.
        let shift = ACTION_BAR_LABEL_SHIFT * (1.0 - reveal);
        let text = entry.label.size();
        let label_x =
            entry.rect.x0 + size.item_padding_x() + size.icon_size() + ACTION_BAR_LABEL_GAP - shift;
        let label_y = entry.rect.y0 + (entry.rect.height() - text.height) / 2.0;
        let layered = reveal < 1.0;
        if layered {
            scene.push_layer(
                origin + Vec2::new(entry.rect.x0, entry.rect.y0),
                entry.rect.size(),
                reveal as f32,
            );
        }
        entry
            .label
            .paint(origin + Vec2::new(label_x, label_y), ink, scene);
        if let Some(shortcut) = &entry.shortcut {
            let hint = shortcut.size();
            shortcut.paint(
                origin
                    + Vec2::new(
                        label_x + text.width + ACTION_BAR_SHORTCUT_GAP,
                        entry.rect.y0 + (entry.rect.height() - hint.height) / 2.0,
                    ),
                style::disabled_tint(chrome.dim_ink, entry.disabled, ACTION_BAR_DISABLED_OPACITY),
                scene,
            );
        }
        if layered {
            scene.pop_layer();
        }
    }

    /// The `Widget::event` key arm: the arrows walk the bar, Enter and Space
    /// activate.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &frust::authoring::KeyEvent) -> EventResult {
        if !ctx.has_focus() {
            return EventResult::Ignored;
        }
        let step = match &key.key {
            Key::Named(NamedKey::ArrowRight) => 1,
            Key::Named(NamedKey::ArrowLeft) => -1,
            _ => 0,
        };
        if step != 0 {
            if self.set_highlight(self.step_highlight(step)) {
                ctx.request_redraw();
            }
            return EventResult::Handled;
        }
        if is_activation_key(key)
            && let Some(index) = self.highlight
        {
            if self.entries.get(index).is_some_and(|e| !e.disabled) {
                (self.on_action)(ctx, index);
            } else if self.set_highlight(None) {
                ctx.request_redraw();
            }
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    /// The `Widget::event` pointer arm.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Ignored;
        }
        let over = inside(p.position, ctx.size());
        match p.phase {
            PointerPhase::Move => {
                if !over {
                    return EventResult::Ignored;
                }
                // Claimed on every qualifying move, and latched, per the
                // three-part hover rule.
                ctx.claim_hover();
                if !self.hovered {
                    self.hovered = true;
                    ctx.request_redraw();
                }
                let item = self
                    .item_at(p.position)
                    .or_else(|| self.resting_highlight());
                if item.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.set_highlight(item) {
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                let Some(index) = self.item_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed = Some(index);
                ctx.capture_pointer();
                // Focus is what routes the arrows here, and what keeps the bar
                // expanded for a keyboard user.
                ctx.request_focus();
                if self.set_highlight(Some(index)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if self.item_at(p.position) == Some(armed) {
                    (self.on_action)(ctx, armed);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                if self.armed.take().is_none() {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Key, KeyEvent, Modifiers, NamedKey};
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(600.0, 200.0);

    /// Long enough for the reveal run and the highlight spring to settle.
    const SETTLE_MS: f64 = 2_000.0;

    #[derive(Default)]
    struct App {
        actions: Vec<usize>,
        expansions: Vec<bool>,
        controlled: Option<bool>,
        disable_last: bool,
    }

    /// A stand-in icon: the catalog ships no icon vocabulary, so an item's mark
    /// is a caller-supplied view.
    fn icon<State: 'static>() -> impl View<State> {
        SizedBox(Some(16.0), Some(16.0))
    }

    fn items(state: &App) -> Vec<ActionBarItem<App>> {
        vec![
            action_bar_item(icon::<App>(), "Reply").active(true),
            action_bar_item(icon::<App>(), "Forward").shortcut("⌘F"),
            action_bar_item(icon::<App>(), "Archive").disabled(state.disable_last),
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
                let mut bar =
                    expandable_action_bar(items(s), |s: &mut App, index| s.actions.push(index))
                        .on_expanded_change(|s: &mut App, expanded| s.expansions.push(expanded));
                if let Some(expanded) = s.controlled {
                    bar = bar.expanded(expanded);
                }
                frust::Stack(vec![any(bar)])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        /// Three frames: one to stage the run, one to let it settle, and one
        /// for the layout that applies it. The reveal is recorded at paint and
        /// read by the *next* layout, so a settled width always trails a
        /// settled reveal by a frame — the one-frame lag the module docs
        /// describe.
        fn settle(&mut self) {
            self.step(SETTLE_MS);
            self.step(SETTLE_MS);
            self.step(1.0);
        }

        fn read_after(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(self.clock));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// The track's box, which is the bar's own size.
        fn track(&mut self) -> Rect {
            let rec = self.read_after(0.0);
            let (origin, size, _, _) = *rec.rrects.first().expect("the track");
            Rect::from_origin_size(origin, size)
        }

        /// Rest the pointer on the first item.
        fn hover_first(&mut self) {
            self.event(pointer(PointerPhase::Move, 20.0, 22.0));
            self.settle();
        }

        /// Move the pointer off the bar entirely.
        fn leave(&mut self) {
            self.event(pointer(PointerPhase::Move, 500.0, 180.0));
        }

        fn key(&mut self, key: NamedKey) {
            self.event(InputEvent::Key(KeyEvent {
                key: Key::Named(key),
                modifiers: Modifiers::default(),
                repeat: false,
            }));
            self.settle();
        }
    }

    /// The width three collapsed items lay out to.
    fn collapsed_width(size: ActionBarSize) -> f64 {
        size.padding() * 2.0 + size.item_min_width() * 3.0 + size.gap() * 2.0
    }

    // ---- Layout -----------------------------------------------------------

    #[test]
    fn the_collapsed_bar_is_a_row_of_square_items() {
        let mut h = Harness::new();
        let track = h.track();
        assert_eq!(track.width(), collapsed_width(ActionBarSize::Md));
        assert_eq!(track.height(), ActionBarSize::Md.min_height());
    }

    /// The resting reveal has to *stay* resting: an idle collapsed bar runs its
    /// stagger clock like any other, and a run whose resting direction did not
    /// match its state would open the bar on its own once time passed.
    #[test]
    fn a_collapsed_bar_stays_collapsed_however_much_time_passes() {
        let mut h = Harness::new();
        h.settle();
        h.settle();
        assert_eq!(h.track().width(), collapsed_width(ActionBarSize::Md));
    }

    #[test]
    fn the_size_rungs_carry_upstreams_own_metrics() {
        assert_eq!(ActionBarSize::Md.item_height(), 32.0);
        assert_eq!(ActionBarSize::Md.icon_size(), 16.0);
        assert_eq!(ActionBarSize::Md.min_height(), 44.0);
        assert_eq!(ActionBarSize::Sm.item_height(), 28.0);
        assert_eq!(ActionBarSize::Sm.icon_size(), 14.0);
        assert_eq!(ActionBarSize::Sm.min_height(), 36.0);
        assert!(ActionBarSize::Sm.padding() < ActionBarSize::Md.padding());
    }

    // ---- Expansion --------------------------------------------------------

    #[test]
    fn a_hover_expands_the_bar_and_the_labels_take_their_own_width() {
        let mut h = Harness::new();
        let collapsed = h.track().width();
        h.hover_first();
        let expanded = h.track().width();
        assert!(
            expanded > collapsed,
            "the labels opened the rail: {collapsed} -> {expanded}"
        );
        // Each item grew by its own label, so the bar is wider than three
        // labelless items by more than the label gap alone.
        assert!(expanded > collapsed + ACTION_BAR_LABEL_GAP * 3.0);
    }

    #[test]
    fn the_bar_holds_open_for_the_collapse_delay_after_the_pointer_leaves() {
        let mut h = Harness::new();
        h.hover_first();
        let expanded = h.track().width();
        h.leave();

        // Inside the delay: still open.
        h.step(ACTION_BAR_COLLAPSE_DELAY.as_secs_f64() * 1_000.0 / 2.0);
        h.step(1.0);
        assert_eq!(h.track().width(), expanded, "still open inside the delay");

        // Past it: the collapse runs.
        h.step(ACTION_BAR_COLLAPSE_DELAY.as_secs_f64() * 1_000.0);
        h.settle();
        assert_eq!(
            h.track().width(),
            collapsed_width(ActionBarSize::Md),
            "collapsed once the delay ran out"
        );
    }

    #[test]
    fn the_expansion_is_reported_one_event_pass_late() {
        let mut h = Harness::new();
        h.hover_first();
        assert!(
            h.state.expansions.is_empty(),
            "the paint that noticed it cannot report it"
        );
        // The next event pass drains the latch.
        h.event(pointer(PointerPhase::Move, 24.0, 22.0));
        assert_eq!(h.state.expansions, vec![true]);
    }

    #[test]
    fn a_controlled_bar_shows_what_it_is_told_and_ignores_the_pointer() {
        let mut h = Harness::new();
        h.state.controlled = Some(true);
        h.settle();
        let expanded = h.track().width();
        assert!(expanded > collapsed_width(ActionBarSize::Md));

        // A pointer resting on a controlled-closed bar does not open it.
        h.state.controlled = Some(false);
        h.settle();
        h.hover_first();
        assert_eq!(h.track().width(), collapsed_width(ActionBarSize::Md));
        assert!(
            h.state.expansions.is_empty(),
            "a controlled flag is the app's own value coming back down"
        );
    }

    #[test]
    fn the_labels_arrive_staggered_and_reduce_motion_lands_them_together() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 20.0, 22.0));
        h.step(0.0);
        let mid = h.read_after(ACTION_BAR_STAGGER.as_secs_f64() * 1_000.0 * 1.5);
        assert!(
            mid.layers.len() < 3,
            "the last item has not started revealing: {:?}",
            mid.layers
        );

        let mut reduced = Harness::themed(reduced());
        reduced.event(pointer(PointerPhase::Move, 20.0, 22.0));
        reduced.step(0.0);
        let flat = reduced.read_after(ACTION_BAR_STAGGER.as_secs_f64() * 1_000.0 * 1.5);
        assert_eq!(flat.layers.len(), 3, "every label is moving at once");
        assert!(
            flat.layers.iter().all(|alpha| *alpha == flat.layers[0]),
            "...and all at the same alpha: {:?}",
            flat.layers
        );
    }

    // ---- Activation -------------------------------------------------------

    #[test]
    fn a_click_on_an_item_reports_it_and_a_release_off_it_does_not() {
        let mut h = Harness::new();
        h.hover_first();
        let track = h.track();
        let first = Point::new(track.x0 + 20.0, track.center().y);
        let elsewhere = Point::new(track.x1 - 10.0, track.center().y);

        h.event(pointer(PointerPhase::Down, first.x, first.y));
        h.event(pointer(PointerPhase::Up, elsewhere.x, elsewhere.y));
        assert!(h.state.actions.is_empty(), "armed-and-re-hit");

        h.event(pointer(PointerPhase::Down, first.x, first.y));
        h.event(pointer(PointerPhase::Up, first.x, first.y));
        assert_eq!(h.state.actions, vec![0]);
    }

    #[test]
    fn a_disabled_item_neither_activates_nor_takes_the_keyboard_walk() {
        let mut h = Harness::new();
        h.state.disable_last = true;
        h.settle();
        h.hover_first();
        let track = h.track();
        // The last item's own box, which is disabled.
        let last = Point::new(track.x1 - 20.0, track.center().y);
        h.event(pointer(PointerPhase::Down, last.x, last.y));
        h.event(pointer(PointerPhase::Up, last.x, last.y));
        assert!(h.state.actions.is_empty(), "a disabled item is inert");

        // Focus the bar with a press on an enabled item, then walk past the
        // disabled one: the walk wraps rather than stopping on it.
        let first = Point::new(track.x0 + 20.0, track.center().y);
        h.event(pointer(PointerPhase::Down, first.x, first.y));
        h.event(pointer(PointerPhase::Up, first.x, first.y));
        h.state.actions.clear();
        h.key(NamedKey::ArrowRight);
        h.key(NamedKey::ArrowRight);
        h.event(InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        }));
        assert_eq!(
            h.state.actions,
            vec![0],
            "the second step wrapped past the disabled item back to the first"
        );
    }

    #[test]
    fn the_arrows_walk_the_bar_and_enter_activates_the_highlight() {
        let mut h = Harness::new();
        let track = h.track();
        let first = Point::new(track.x0 + 20.0, track.center().y);
        // A press claims the focus the arrows route by.
        h.event(pointer(PointerPhase::Down, first.x, first.y));
        h.event(pointer(PointerPhase::Up, first.x, first.y));
        h.state.actions.clear();
        h.key(NamedKey::ArrowRight);
        h.event(InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        }));
        assert_eq!(h.state.actions, vec![1]);
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn the_bar_paints_its_track_its_highlight_and_every_label() {
        let mut h = Harness::new();
        h.hover_first();
        let rec = h.read_after(0.0);
        assert!(!rec.shadows.is_empty(), "the track casts the panel shadow");
        assert!(rec.strokes > 0, "and strokes its hairline");
        let track_radius = ActionBarSize::Md.min_height() / 2.0;
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, r, _)| (*r - track_radius).abs() < 0.001),
            "the track is a pill"
        );
        let item_radius = ActionBarSize::Md.item_height() / 2.0;
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, r, _)| (*r - item_radius).abs() < 0.001),
            "the highlight is a pill on the hovered item"
        );
        // Three labels and one shortcut.
        assert!(rec.inks.len() >= 4, "every run painted: {}", rec.inks.len());
    }
}
