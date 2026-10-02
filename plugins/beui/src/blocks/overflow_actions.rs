//! Ports beUI's `overflow-actions` block — `components/motion/overflow-actions.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `overflow-actions`: *"Connected pill rail for primary actions
//! that springs open to reveal extra controls."*
//!
//! | upstream | here |
//! |---|---|
//! | track `rounded-full border border-border bg-card overflow-hidden` | the shared panel chrome at [`style::RADIUS_CONTROL`], clipped |
//! | `gap-1.5 p-1.5 text-sm` / `gap-1 p-1 text-xs` | [`OverflowSize::Md`] / [`OverflowSize::Sm`] |
//! | action `h-9 min-w-9 px-3.5 rounded-full bg-background` / `h-8 min-w-8 px-3` | [`OverflowSize::action_height`], [`OverflowSize::action_padding_x`] |
//! | toggle `h-9 w-9 rounded-full bg-primary text-primary-foreground` | [`OverflowSize::toggle_size`] |
//! | `SHELL_TRANSITION` `{stiffness 220, damping 17, mass 0.85}` | [`OVERFLOW_SHELL`] |
//! | `ICON_VARIANTS` `{opacity, blur(3px)}` over `0.18` | [`OVERFLOW_ICON_SWAP`] (fade only — no blur primitive) |
//! | `OVERFLOW_ACTION_VARIANTS` `{opacity, blur(4px)}` | the per-item reveal |
//! | `MoreHorizontal` → `X` | [`draw_more`] → [`draw_close`] |
//! | `disabled:opacity-45` | [`OVERFLOW_DISABLED_OPACITY`] |
//! | `collapseOnAction` | [`OverflowActionsView::collapse_on_action`] |
//!
//! # Two premise corrections, and the width math that was asked for
//!
//! The porting card describes this component as *"an overflow ('…') menu with an
//! anchored panel and staggered items"* that *"measures available width and
//! collapses extra actions into overflow when constrained"*. Upstream is neither
//! of those things: it reveals its extra actions **inline**, inside the same
//! pill track, with no panel and no portal at all, and the split between primary
//! and overflow actions is an **explicit prop pair** (`primaryActions` /
//! `overflowActions`) rather than a measurement. The port follows upstream on
//! both counts — an anchored panel would be a different component, and
//! [`crate::components::context_menu`] already is that component.
//!
//! What the card asks for on top of upstream is genuinely additive and is
//! implemented as such: the rail also **measures**. Its layout shapes every
//! primary action's label, resolves each one's own width, and fits as many as
//! the incoming constraint allows — the arithmetic is [`overflow_split`], a pure
//! function of the measured widths, the gap, the reserved chrome (the track's
//! padding plus the toggle) and the available width. Anything that does not fit
//! is pushed into the overflow group *ahead of* the declared overflow actions,
//! so one toggle reveals both. A rail with room for everything behaves exactly
//! as upstream's does.
//!
//! The measurement runs where the widths exist — in `layout`, from the shaped
//! labels — and never in `paint` or an event pass, so it cannot disagree with
//! the boxes actually laid out. The reveal, which changes those boxes, is the
//! [`crate::blocks::expandable_action_bar`] arrangement: advanced at paint,
//! recorded, applied by the next layout.
//!
//! # The stagger is an addition
//!
//! Upstream fades its overflow items in together (`OVERFLOW_ACTION_VARIANTS` has
//! no per-child delay). The card asks for a stagger, so they arrive
//! [`OVERFLOW_STAGGER`] apart and unwind on
//! [`crate::motion::stagger::EXIT_DELAY_FACTOR`]'s halved delays;
//! `reduce_motion` collapses it back to one beat.
//!
//! # Degradations against the web original
//!
//! - **No icons.** `OverflowActionItem.icon` is an arbitrary node. Unlike
//!   [`crate::blocks::expandable_action_bar`], whose collapsed state *is* its
//!   icons, this rail is legible as labels alone, so it takes no icon child and
//!   keeps its width math free of a child measurement it cannot shape itself.
//! - **No blur on the fades.** `filter: blur(3px)`/`blur(4px)` have no
//!   `PaintScene` primitive; the opacity halves of both variants are kept.
//! - **The toggle has no press or hover scale.** Upstream's `whileTap`/
//!   `whileHover` scale the toggle by `0.96`/`1.03`; the press treatment here is
//!   the catalog's own ([`style::PRESS_SCALE_CSS`]-shaped fill change), since a
//!   scaled toggle inside an `overflow-hidden` track clips against its own
//!   edge.
//! - **A rail wider than its constraint clips rather than scrolling**, which is
//!   upstream's own `overflow-hidden` track behaviour.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2, View, Widget,
    erase_callback_arg, text::TextStyle,
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::components::popover::{PanelChrome, paint_panel, resolve_panel};
use crate::motion::stagger::StaggerDirection;
use crate::motion::{Ramp, Stagger};
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// The two size rungs upstream ships (`OverflowActionsSize`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverflowSize {
    /// `gap-1 p-1 text-xs`, actions `h-8 min-w-8 px-3`, toggle `h-8 w-8`.
    Sm,
    /// `gap-1.5 p-1.5 text-sm`, actions `h-9 min-w-9 px-3.5`, toggle `h-9 w-9`.
    #[default]
    Md,
}

impl OverflowSize {
    /// `p-1` / `p-1.5` — the track's own padding, in logical px.
    pub const fn padding(self) -> f64 {
        match self {
            OverflowSize::Sm => 4.0,
            OverflowSize::Md => 6.0,
        }
    }

    /// `gap-1` / `gap-1.5` — the gap between rail entries, in logical px.
    pub const fn gap(self) -> f64 {
        self.padding()
    }

    /// `h-8` / `h-9` — one action's height, in logical px.
    pub const fn action_height(self) -> f64 {
        match self {
            OverflowSize::Sm => 32.0,
            OverflowSize::Md => 36.0,
        }
    }

    /// `min-w-8` / `min-w-9` — one action's least width, in logical px.
    pub const fn action_min_width(self) -> f64 {
        self.action_height()
    }

    /// `px-3` / `px-3.5` — an action's horizontal padding, in logical px.
    pub const fn action_padding_x(self) -> f64 {
        match self {
            OverflowSize::Sm => 12.0,
            OverflowSize::Md => 14.0,
        }
    }

    /// `h-8 w-8` / `h-9 w-9` — the toggle's box, in logical px.
    pub const fn toggle_size(self) -> f64 {
        self.action_height()
    }

    /// `h-3.5` / `h-4` — the toggle mark's box, in logical px.
    pub const fn icon_size(self) -> f64 {
        match self {
            OverflowSize::Sm => 14.0,
            OverflowSize::Md => 16.0,
        }
    }

    /// `text-xs` / `text-sm` — an action label's size, in logical px.
    pub const fn text_size(self) -> f64 {
        match self {
            OverflowSize::Sm => style::TEXT_XS,
            OverflowSize::Md => style::TEXT_SM,
        }
    }
}

/// `disabled:opacity-45` — a disabled action's ink opacity.
pub const OVERFLOW_DISABLED_OPACITY: f32 = 0.45;

/// The alpha the toggle's fill takes while it holds a press — the catalog's own
/// press treatment, standing in for upstream's `whileTap` scale (see the
/// [module docs](self)).
pub const OVERFLOW_TOGGLE_PRESS_ALPHA: f32 = 0.85;

// ---- Motion ----------------------------------------------------------------

/// `SHELL_TRANSITION` — the rail's own resize, *"a softer layout spring than the
/// app defaults so the overflow group stays visually attached to the toggle"*.
pub const OVERFLOW_SHELL: SpringDescription = SpringDescription {
    mass: 0.85,
    stiffness: 220.0,
    damping: 17.0,
};

/// `ICON_VARIANTS`' duration — how long the toggle's mark takes to swap.
pub const OVERFLOW_ICON_SWAP: Duration = Duration::from_millis(180);

/// How far apart the overflow actions reveal. An addition — see the [module
/// docs](self).
pub const OVERFLOW_STAGGER: Duration = Duration::from_millis(30);

// ---- Items -----------------------------------------------------------------

/// One action on the rail — upstream's `OverflowActionItem`, minus the icon the
/// [module docs](self) record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverflowActionItem {
    label: String,
    disabled: bool,
}

/// An action showing `label`.
pub fn overflow_action(label: impl Into<String>) -> OverflowActionItem {
    OverflowActionItem {
        label: label.into(),
        disabled: false,
    }
}

impl OverflowActionItem {
    /// Make this action unactivatable (`disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// This action's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// How many of `widths` (in rail order) fit in `available`, given the `gap`
/// after each one and the `reserved` chrome the track always carries — its
/// padding on both sides plus the toggle.
///
/// The rail's collapse arithmetic, split out as a pure function so it is
/// testable at synthetic widths without laying anything out. `n` entries cost
/// `sum(widths[..n]) + gap * n`: each one is followed by a gap, before the next
/// entry or before the toggle.
///
/// An unbounded (infinite or NaN) `available` fits everything, which is what an
/// unconstrained mount reads. A rail with room for nothing returns `0` — the
/// toggle alone is a legitimate rail, and reserving one visible action would
/// overflow the very constraint this measures against.
pub fn overflow_split(widths: &[f64], gap: f64, reserved: f64, available: f64) -> usize {
    if !available.is_finite() {
        return widths.len();
    }
    let mut used = reserved;
    for (fitted, width) in widths.iter().enumerate() {
        used += width + gap;
        if used > available {
            return fitted;
        }
    }
    widths.len()
}

// ---- The component ---------------------------------------------------------

/// A view-held action callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State, usize)>;

/// A view-held expansion callback (erased on build).
type OnExpandedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// What the rail renders from, beyond its two item lists.
#[derive(Clone, Debug, PartialEq)]
struct RailConfig {
    /// The controlled override, when the app owns the expansion.
    expanded: Option<bool>,
    collapse_on_action: bool,
    size: OverflowSize,
    open_label: String,
    close_label: String,
}

/// A declarative beUI overflow-actions rail. See [`overflow_actions`].
pub struct OverflowActionsView<State: 'static> {
    primary: Vec<OverflowActionItem>,
    overflow: Vec<OverflowActionItem>,
    config: RailConfig,
    on_action: OnAction<State>,
    on_expanded_change: OnExpandedChange<State>,
}

/// Build an action rail: `primary` actions always on the rail (as far as the
/// width allows), `overflow` actions behind the toggle.
///
/// `on_action(state, index)` addresses **one concatenated list**: `0..primary
/// .len()` are the primary actions in order, and the indices above that are the
/// overflow actions. An action the width pushed into the overflow group keeps
/// its own primary index — the split is a layout decision, never a renumbering.
pub fn overflow_actions<State: 'static, F: Fn(&mut State, usize) + 'static>(
    primary: Vec<OverflowActionItem>,
    overflow: Vec<OverflowActionItem>,
    on_action: F,
) -> OverflowActionsView<State> {
    OverflowActionsView {
        primary,
        overflow,
        config: RailConfig {
            expanded: None,
            collapse_on_action: false,
            size: OverflowSize::default(),
            open_label: "Show extra actions".to_string(),
            close_label: "Hide extra actions".to_string(),
        },
        on_action: Rc::new(on_action),
        on_expanded_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> OverflowActionsView<State> {
    /// Take the expansion over: the rail shows exactly what `expanded` says and
    /// its toggle only reports (upstream's controlled `expanded` prop).
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.config.expanded = Some(expanded);
        self
    }

    /// Collapse the rail after an action fires (`collapseOnAction`).
    pub fn collapse_on_action(mut self, collapse_on_action: bool) -> Self {
        self.config.collapse_on_action = collapse_on_action;
        self
    }

    /// Pick the size rung (default [`OverflowSize::Md`]).
    pub fn size(mut self, size: OverflowSize) -> Self {
        self.config.size = size;
        self
    }

    /// Set the toggle's accessible name while collapsed (`openLabel`).
    pub fn open_label(mut self, label: impl Into<String>) -> Self {
        self.config.open_label = label.into();
        self
    }

    /// Set the toggle's accessible name while expanded (`closeLabel`).
    pub fn close_label(mut self, label: impl Into<String>) -> Self {
        self.config.close_label = label.into();
        self
    }

    /// Set the expansion callback, fired from the toggle's own press.
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
    disabled: bool,
    /// The action's index in the concatenated addressing `on_action` reports.
    index: usize,
    /// The action's own full width once shaped, in logical px.
    width: f64,
    /// The action's box in the rail's own space, or `Rect::ZERO` while it is
    /// fully hidden.
    rect: Rect,
}

/// The retained widget for an [`OverflowActionsView`].
pub struct OverflowActionsWidget {
    /// The declared primary actions, in order.
    primary: Vec<Entry>,
    /// The declared overflow actions, in order.
    overflow: Vec<Entry>,
    config: RailConfig,
    /// How many primary actions the last layout could fit on the rail.
    fitted: usize,
    /// Each hidden action's reveal as of the last paint, indexed the way
    /// [`OverflowActionsWidget::hidden_order`] orders them.
    reveals: Vec<f64>,
    /// Whether the rail is showing its overflow group.
    expanded: bool,
    /// The frame the current reveal run started on.
    run_started: Option<FrameTime>,
    /// Which way that run is travelling.
    direction: StaggerDirection,
    /// The toggle's mark crossfade: `0.0` the dots, `1.0` the close mark.
    mark: Lane,
    /// The toggle's box in the rail's own space.
    toggle: Rect,
    /// Whether the toggle is holding a press.
    toggle_pressed: bool,
    /// The action a primary `Down` armed, by concatenated index.
    armed: Option<usize>,
    on_action: ErasedArgCallback<usize>,
    on_expanded_change: ErasedArgCallback<bool>,
}

impl OverflowActionsWidget {
    /// Whether the rail is showing its overflow group.
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// How many primary actions the last layout kept on the rail. The rest were
    /// pushed into the overflow group — see [`overflow_split`].
    pub fn fitted(&self) -> usize {
        self.fitted
    }

    /// The toggle's box in the rail's own space.
    pub fn toggle_rect(&self) -> Rect {
        self.toggle
    }

    /// How many actions sit behind the toggle: the primary actions the width
    /// pushed out, plus the declared overflow ones. They are addressed by slot,
    /// in that order, through [`hidden`](Self::hidden).
    fn hidden_count(&self) -> usize {
        self.primary.len().saturating_sub(self.fitted) + self.overflow.len()
    }

    /// One hidden entry by its position in [`hidden_order`](Self::hidden_order).
    fn hidden(&self, slot: usize) -> Option<&Entry> {
        let pushed = self.primary.len().saturating_sub(self.fitted);
        if slot < pushed {
            self.primary.get(self.fitted + slot)
        } else {
            self.overflow.get(slot - pushed)
        }
    }

    /// The same, mutably.
    fn hidden_mut(&mut self, slot: usize) -> Option<&mut Entry> {
        let pushed = self.primary.len().saturating_sub(self.fitted);
        if slot < pushed {
            self.primary.get_mut(self.fitted + slot)
        } else {
            self.overflow.get_mut(slot - pushed)
        }
    }

    /// One hidden action's reveal as of the last paint.
    fn reveal(&self, slot: usize) -> f64 {
        self.reveals.get(slot).copied().unwrap_or(0.0)
    }

    /// The stagger the overflow group reveals on.
    fn stagger(&self, reduce: bool) -> Stagger {
        let base =
            Stagger::new(OVERFLOW_STAGGER, Ramp::spring(OVERFLOW_SHELL)).direction(self.direction);
        if reduce { base.collapsed() } else { base }
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
        self.mark.retarget_with(
            Ramp::eased(OVERFLOW_ICON_SWAP, EASE_OUT),
            if expanded { 1.0 } else { 0.0 },
        );
    }

    /// The rail entry `position` lands on, as a concatenated index.
    fn action_at(&self, position: Point) -> Option<usize> {
        self.primary
            .iter()
            .chain(self.overflow.iter())
            .find(|entry| {
                !entry.disabled && entry.rect.width() > 0.0 && entry.rect.contains(position)
            })
            .map(|entry| entry.index)
    }

    /// Fire `index`, and collapse afterwards when the rail was told to.
    fn fire(&mut self, ctx: &mut EventCtx, index: usize) {
        (self.on_action)(ctx, index);
        if self.config.collapse_on_action && self.expanded && self.config.expanded.is_none() {
            self.set_expanded(false);
            (self.on_expanded_change)(ctx, false);
            // The event pass cannot ask for a relayout; the reveal run this
            // just restarted is unsettled, so the next paint asks for one.
            ctx.request_redraw();
        }
    }

    /// Flip the rail open or shut from the toggle's own press.
    fn toggle(&mut self, ctx: &mut EventCtx) {
        let next = !self.expanded;
        // A controlled rail reports the request and waits for the app's value.
        if self.config.expanded.is_none() {
            self.set_expanded(next);
            // As above: `EventCtx` carries no relayout request, and the restarted
            // run makes the next paint ask for one.
            ctx.request_redraw();
        }
        (self.on_expanded_change)(ctx, next);
    }
}

impl<State: 'static> View<State> for OverflowActionsView<State> {
    type Element = OverflowActionsWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> OverflowActionsWidget {
        let expanded = self.config.expanded.unwrap_or(false);
        let primary = entries(&self.primary, 0);
        let overflow = entries(&self.overflow, self.primary.len());
        OverflowActionsWidget {
            reveals: vec![if expanded { 1.0 } else { 0.0 }; overflow.len()],
            primary,
            overflow,
            config: self.config.clone(),
            fitted: self.primary.len(),
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
            mark: Lane::at_rest(
                Ramp::eased(OVERFLOW_ICON_SWAP, EASE_OUT),
                if expanded { 1.0 } else { 0.0 },
            ),
            toggle: Rect::ZERO,
            toggle_pressed: false,
            armed: None,
            on_action: erase_callback_arg(&self.on_action),
            on_expanded_change: erase_callback_arg(&self.on_expanded_change),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut OverflowActionsWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if sync_entries(&mut element.primary, &self.primary, 0)
            | sync_entries(&mut element.overflow, &self.overflow, self.primary.len())
        {
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.config != self.config {
            if element.config.size != self.config.size {
                flags |= ChangeFlags::LAYOUT;
            }
            element.config = self.config.clone();
            if let Some(expanded) = self.config.expanded {
                element.set_expanded(expanded);
                flags |= ChangeFlags::LAYOUT;
            }
            flags |= ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_action = erase_callback_arg(&self.on_action);
        element.on_expanded_change = erase_callback_arg(&self.on_expanded_change);
        flags
    }

    fn teardown(&self, _element: &mut OverflowActionsWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// Build the retained entries for one declared list, numbered from `base`.
fn entries(items: &[OverflowActionItem], base: usize) -> Vec<Entry> {
    items
        .iter()
        .enumerate()
        .map(|(offset, item)| Entry {
            label: LabelRun::new(item.label.clone()),
            disabled: item.disabled,
            index: base + offset,
            width: 0.0,
            rect: Rect::ZERO,
        })
        .collect()
}

/// Adopt a declared list into `slots`, reporting whether the layout has to run
/// again.
fn sync_entries(slots: &mut Vec<Entry>, items: &[OverflowActionItem], base: usize) -> bool {
    if slots.len() != items.len() {
        *slots = entries(items, base);
        return true;
    }
    let mut changed = false;
    for (slot, item) in slots.iter_mut().zip(items) {
        changed |= slot.label.set_content(item.label.clone());
        changed |= slot.disabled != item.disabled;
        slot.disabled = item.disabled;
    }
    changed
}

/// An action label's style at `size`, in the theme's `label_large` family.
fn action_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    themed_style(
        crate::text::label_style(size),
        ThemeTextType::LabelLarge,
        theme,
    )
}

/// Paint lucide's `more-horizontal` mark — three dots — centred on `centre`.
pub fn draw_more(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    // `circle cx=12 cy=12 r=1`, `cx=19`, `cx=5`, relative to the box's centre.
    for dx in [-7.0, 0.0, 7.0] {
        scene.fill_rounded_rect(
            Point::new(centre.x + (dx - 1.0) * scale, centre.y - scale),
            Size::new(2.0 * scale, 2.0 * scale),
            scale,
            color,
        );
    }
}

/// Paint lucide's `x` mark — two strokes — centred on `centre`.
pub fn draw_close(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let arm = 6.0 * scale;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, -arm));
    path.line_to(Point::new(arm, arm));
    path.move_to(Point::new(arm, -arm));
    path.line_to(Point::new(-arm, arm));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// lucide's own viewBox extent.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// lucide's default `strokeWidth`, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

impl Widget for OverflowActionsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let size = self.config.size;
        let style = action_style(theme, size.text_size());

        // Every action's own width, from its shaped label — the measurement the
        // split below is taken against.
        for entry in self.primary.iter_mut().chain(self.overflow.iter_mut()) {
            let label = entry.label.layout(ctx, &style).width;
            entry.width = (label + size.action_padding_x() * 2.0).max(size.action_min_width());
            entry.rect = Rect::ZERO;
        }

        let height = size.action_height() + size.padding() * 2.0;
        let reserved = size.padding() * 2.0 + size.toggle_size();
        let widths: Vec<f64> = self.primary.iter().map(|entry| entry.width).collect();
        self.fitted = overflow_split(&widths, size.gap(), reserved, bc.max().width);

        // The rail: the fitted primaries, then whatever the reveal is showing of
        // the hidden group, then the toggle.
        let top = size.padding();
        let mut x = size.padding();
        for index in 0..self.fitted {
            let entry = &mut self.primary[index];
            entry.rect = Rect::new(x, top, x + entry.width, top + size.action_height());
            x += entry.width + size.gap();
        }
        let hidden = self.hidden_count();
        self.reveals.resize(hidden, 0.0);
        for slot in 0..hidden {
            let reveal = self.reveal(slot).clamp(0.0, 1.0);
            let Some(entry) = self.hidden_mut(slot) else {
                continue;
            };
            let width = entry.width * reveal;
            if width <= 0.0 {
                entry.rect = Rect::ZERO;
                continue;
            }
            entry.rect = Rect::new(x, top, x + width, top + size.action_height());
            x += width + size.gap() * reveal;
        }
        self.toggle = Rect::new(x, top, x + size.toggle_size(), top + size.action_height());
        let width = self.toggle.x1 + size.padding();
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let (accent, on_accent, surface) = match theme {
            Some(t) => (
                t.scheme().primary,
                t.scheme().on_primary,
                t.scheme().surface,
            ),
            None => (
                crate::BEUI_LIGHT.primary,
                crate::BEUI_LIGHT.primary_foreground,
                crate::BEUI_LIGHT.background,
            ),
        };
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = self.config.size;

        // The track, and its `overflow-hidden` clip.
        let track = Rect::from_origin_size(Point::ORIGIN, ctx.size());
        let radius = style::resolve_radius(style::RADIUS_CONTROL, track.width(), track.height());
        paint_panel(scene, origin, track, radius, chrome);
        scene.push_clip_rounded(origin, ctx.size(), radius);

        // The reveal run — recorded for the next layout, which is where a width
        // change actually lands.
        let started = *self.run_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let stagger = self.stagger(reduce);
        let count = self.hidden_count();
        if !stagger.is_settled(elapsed, count) {
            ctx.request_layout();
        }
        self.reveals = (0..count)
            .map(|slot| stagger.revealed(elapsed, slot, count))
            .collect();

        for index in 0..self.fitted {
            paint_action(scene, origin, &self.primary[index], chrome, surface, 1.0);
        }
        for slot in 0..count {
            let reveal = self.reveal(slot).clamp(0.0, 1.0);
            if reveal <= 0.0 {
                continue;
            }
            if let Some(entry) = self.hidden(slot) {
                paint_action(scene, origin, entry, chrome, surface, reveal);
            }
        }

        // The toggle, and its crossfading mark.
        if self.mark.advance(now) {
            ctx.request_frame();
        }
        let swap = self.mark.value().clamp(0.0, 1.0);
        let fill = if self.toggle_pressed {
            style::scale_alpha(accent, OVERFLOW_TOGGLE_PRESS_ALPHA)
        } else {
            accent
        };
        scene.fill_rounded_rect(
            origin + self.toggle.origin().to_vec2(),
            self.toggle.size(),
            style::resolve_radius(
                style::RADIUS_CONTROL,
                self.toggle.width(),
                self.toggle.height(),
            ),
            fill,
        );
        let centre = origin + self.toggle.center().to_vec2();
        if swap < 1.0 {
            draw_more(
                scene,
                centre,
                size.icon_size(),
                style::with_alpha(on_accent, (1.0 - swap) as f32),
            );
        }
        if swap > 0.0 {
            draw_close(
                scene,
                centre,
                size.icon_size(),
                style::with_alpha(on_accent, swap as f32),
            );
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                if is_activation_key(key) {
                    self.toggle(ctx);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if key.key == Key::Named(NamedKey::Escape) && self.expanded {
                    // Escape shuts the group, the same way the toggle does.
                    self.toggle(ctx);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let expanded = self.expanded;
        let fitted = self.fitted;
        let toggle_label = if expanded {
            self.config.close_label.clone()
        } else {
            self.config.open_label.clone()
        };
        ctx.push_container(
            Role::Toolbar,
            |_| {},
            |ctx| {
                for (index, entry) in self.primary.iter().chain(self.overflow.iter()).enumerate() {
                    // An action the width pushed out is only reachable once the
                    // group is open — input parity, the carve-out
                    // `docs/CODE_STANDARDS.md`'s Semantics Conventions name.
                    let on_rail = index < fitted;
                    if !on_rail && !expanded {
                        continue;
                    }
                    let label = entry.label.content().to_string();
                    let disabled = entry.disabled;
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(label.as_str());
                        if disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
                ctx.push_node(Role::Button, |node| {
                    node.set_label(toggle_label.as_str());
                    node.set_expanded(expanded);
                    node.add_action(Action::Click);
                });
            },
        );
    }
}

/// Paint one action pill: `bg-background` under its label, faded by `reveal`.
fn paint_action(
    scene: &mut dyn PaintScene,
    origin: Point,
    entry: &Entry,
    chrome: PanelChrome,
    surface: Color,
    reveal: f64,
) {
    if entry.rect.width() <= 0.0 {
        return;
    }
    let alpha = reveal.clamp(0.0, 1.0) as f32;
    let layered = alpha < 1.0;
    if layered {
        scene.push_layer(
            origin + entry.rect.origin().to_vec2(),
            entry.rect.size(),
            alpha,
        );
    }
    scene.fill_rounded_rect(
        origin + entry.rect.origin().to_vec2(),
        entry.rect.size(),
        style::resolve_radius(
            style::RADIUS_CONTROL,
            entry.rect.width(),
            entry.rect.height(),
        ),
        surface,
    );
    let text = entry.label.size();
    entry.label.paint(
        origin
            + Vec2::new(
                entry.rect.x0 + (entry.rect.width() - text.width) / 2.0,
                entry.rect.y0 + (entry.rect.height() - text.height) / 2.0,
            ),
        style::disabled_tint(chrome.ink, entry.disabled, OVERFLOW_DISABLED_OPACITY),
        scene,
    );
    if layered {
        scene.pop_layer();
    }
}

impl OverflowActionsWidget {
    /// The `Widget::event` pointer arm: the toggle owns its own press, the
    /// actions own theirs.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Ignored;
        }
        if !inside(p.position, ctx.size()) {
            return EventResult::Ignored;
        }
        let on_toggle = self.toggle.contains(p.position);
        let action = self.action_at(p.position);
        match p.phase {
            PointerPhase::Move => {
                if on_toggle || action.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                ctx.capture_pointer();
                // Focus is what routes Escape and the activation keys here.
                ctx.request_focus();
                if on_toggle {
                    self.toggle_pressed = true;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                let Some(index) = action else {
                    return EventResult::Ignored;
                };
                self.armed = Some(index);
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.toggle_pressed {
                    self.toggle_pressed = false;
                    ctx.request_redraw();
                    if on_toggle {
                        self.toggle(ctx);
                    }
                    return EventResult::Handled;
                }
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if action == Some(armed) {
                    self.fire(ctx, armed);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                let held = self.toggle_pressed || self.armed.is_some();
                self.toggle_pressed = false;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::text::TextContext;
    use frust::authoring::{Key, KeyEvent, Modifiers, NamedKey, any};
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(700.0, 200.0);

    /// Long enough for the reveal run and the mark swap to settle.
    const SETTLE_MS: f64 = 2_000.0;

    #[derive(Default)]
    struct App {
        actions: Vec<usize>,
        expansions: Vec<bool>,
        controlled: Option<bool>,
        collapse_on_action: bool,
        /// How far the rail's own box is squeezed from each side.
        squeeze: f64,
    }

    fn primary() -> Vec<OverflowActionItem> {
        vec![
            overflow_action("Share"),
            overflow_action("Duplicate"),
            overflow_action("Rename"),
        ]
    }

    fn overflow() -> Vec<OverflowActionItem> {
        vec![
            overflow_action("Archive"),
            overflow_action("Delete").disabled(true),
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
                let mut rail = overflow_actions(primary(), overflow(), |s: &mut App, index| {
                    s.actions.push(index)
                })
                .collapse_on_action(s.collapse_on_action)
                .on_expanded_change(|s: &mut App, expanded| s.expansions.push(expanded));
                if let Some(expanded) = s.controlled {
                    rail = rail.expanded(expanded);
                }
                // A `Padding` is how the harness hands the rail a synthetic
                // available width: the inset comes straight off the max
                // constraint the rail measures against.
                frust::Stack(vec![any(frust::Padding(
                    frust::EdgeInsets::symmetric(s.squeeze, 0.0),
                    rail,
                ))])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        /// Three frames: stage the run, settle it, let the layout apply it.
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

        /// A whole frame `ms` later, recording what it drew — the read a
        /// mid-run assertion needs, since a reveal only reaches a box through
        /// the layout that follows the paint which computed it.
        fn step_read(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut logic = move |s: &mut App| {
                let mut rail = overflow_actions(primary(), overflow(), |s: &mut App, index| {
                    s.actions.push(index)
                })
                .collapse_on_action(s.collapse_on_action)
                .on_expanded_change(|s: &mut App, expanded| s.expansions.push(expanded));
                if let Some(expanded) = s.controlled {
                    rail = rail.expanded(expanded);
                }
                frust::Stack(vec![any(frust::Padding(
                    frust::EdgeInsets::symmetric(s.squeeze, 0.0),
                    rail,
                ))])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            let now = self.clock;
            self.root.paint(&mut rec, ft_ms(now));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// The track's box, which is the rail's own size.
        fn track(&mut self) -> Rect {
            let rec = self.read_after(0.0);
            let (origin, size, _, _) = *rec.rrects.first().expect("the track");
            Rect::from_origin_size(origin, size)
        }

        /// Every action pill on the rail, in paint order.
        ///
        /// The rail paints the track, then each visible action, then the
        /// toggle — so the action-height boxes after the track are the pills,
        /// and the last of them is the toggle.
        fn pills(&mut self) -> Vec<Rect> {
            let rec = self.read_after(0.0);
            let mut boxes: Vec<Rect> = rec
                .rrects
                .iter()
                .skip(1)
                .filter(|(_, size, _, _)| size.height == OverflowSize::Md.action_height())
                .map(|(origin, size, _, _)| Rect::from_origin_size(*origin, *size))
                .collect();
            boxes.pop();
            boxes
        }

        /// How many action pills the rail is showing.
        fn visible_actions(&mut self) -> usize {
            self.pills().len()
        }

        /// Click the `index`-th visible pill.
        fn click_pill(&mut self, index: usize) {
            let at = self.pills()[index].center();
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
        }

        /// The toggle's centre, in window space.
        fn toggle_point(&mut self) -> Point {
            let track = self.track();
            Point::new(
                track.x1 - OverflowSize::Md.padding() - OverflowSize::Md.toggle_size() / 2.0,
                track.center().y,
            )
        }

        /// Press the toggle.
        fn press_toggle(&mut self) {
            let at = self.toggle_point();
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.settle();
        }
    }

    // ---- The collapse math ------------------------------------------------

    #[test]
    fn the_split_fits_as_many_actions_as_the_width_allows() {
        // Three 100px actions, 8px gaps, 60px of reserved chrome: each one
        // costs 108, so the rail needs 60 + 108n.
        let widths = [100.0, 100.0, 100.0];
        assert_eq!(
            overflow_split(&widths, 8.0, 60.0, 1_000.0),
            3,
            "everything fits"
        );
        assert_eq!(
            overflow_split(&widths, 8.0, 60.0, 384.0),
            3,
            "exactly three"
        );
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 383.0), 2);
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 276.0), 2, "exactly two");
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 275.0), 1);
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 168.0), 1, "exactly one");
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 167.0), 0);
        assert_eq!(
            overflow_split(&widths, 8.0, 60.0, 0.0),
            0,
            "the toggle alone is a legitimate rail"
        );
    }

    #[test]
    fn an_unbounded_width_fits_everything_and_an_empty_rail_splits_at_zero() {
        let widths = [100.0, 100.0];
        assert_eq!(overflow_split(&widths, 8.0, 60.0, f64::INFINITY), 2);
        assert_eq!(overflow_split(&widths, 8.0, 60.0, f64::NAN), 2);
        assert_eq!(overflow_split(&[], 8.0, 60.0, 10.0), 0);
    }

    #[test]
    fn the_split_is_order_preserving_so_the_trailing_actions_are_the_ones_pushed_out() {
        // A wide leading action starves the rest even though they would fit.
        let widths = [200.0, 40.0, 40.0];
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 300.0), 1);
        // Room for the first two, not the third.
        assert_eq!(overflow_split(&widths, 8.0, 60.0, 320.0), 2);
    }

    // ---- The rail ---------------------------------------------------------

    #[test]
    fn a_wide_rail_shows_every_primary_action_and_the_toggle() {
        let mut h = Harness::new();
        h.settle();
        assert_eq!(h.visible_actions(), 3);
    }

    #[test]
    fn a_squeezed_rail_pushes_its_trailing_actions_behind_the_toggle() {
        let mut h = Harness::new();
        h.settle();
        let roomy = h.track().width();

        // Squeeze the available width down to something only the first action
        // can live in.
        h.state.squeeze = (WINDOW.width - 160.0) / 2.0;
        h.settle();
        let squeezed = h.track().width();
        assert!(squeezed < roomy, "{squeezed} < {roomy}");
        assert!(
            h.visible_actions() < 3,
            "the trailing actions moved behind the toggle"
        );
        assert!(
            squeezed <= 160.0 + 0.001,
            "and the rail stayed inside its constraint: {squeezed}"
        );
    }

    #[test]
    fn the_toggle_reveals_the_pushed_out_actions_alongside_the_declared_ones() {
        let mut h = Harness::new();
        h.state.squeeze = (WINDOW.width - 200.0) / 2.0;
        h.settle();
        let collapsed = h.visible_actions();
        h.press_toggle();
        let expanded = h.visible_actions();
        assert!(
            expanded > collapsed,
            "one toggle opens both groups: {collapsed} -> {expanded}"
        );
        assert_eq!(h.state.expansions, vec![true]);
    }

    // ---- The toggle -------------------------------------------------------

    #[test]
    fn the_toggle_expands_the_group_and_reports_it() {
        let mut h = Harness::new();
        h.settle();
        let collapsed = h.track().width();
        h.press_toggle();
        assert_eq!(h.state.expansions, vec![true]);
        assert!(
            h.track().width() > collapsed,
            "the rail sprang open for the overflow group"
        );
        h.press_toggle();
        assert_eq!(h.state.expansions, vec![true, false]);
        assert_eq!(h.track().width(), collapsed, "and shut again");
    }

    #[test]
    fn a_controlled_rail_reports_the_request_and_waits_for_the_app() {
        let mut h = Harness::new();
        h.state.controlled = Some(false);
        h.settle();
        let collapsed = h.track().width();
        h.press_toggle();
        assert_eq!(h.state.expansions, vec![true], "the request is reported");
        assert_eq!(
            h.track().width(),
            collapsed,
            "...and nothing moved until the app said so"
        );

        h.state.controlled = Some(true);
        h.settle();
        assert!(h.track().width() > collapsed);
    }

    #[test]
    fn escape_shuts_an_open_group() {
        let mut h = Harness::new();
        h.settle();
        h.press_toggle();
        h.event(InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        }));
        h.settle();
        assert_eq!(h.state.expansions, vec![true, false]);
    }

    // ---- Actions ----------------------------------------------------------

    #[test]
    fn a_click_reports_the_action_by_its_concatenated_index() {
        let mut h = Harness::new();
        h.settle();
        h.click_pill(0);
        assert_eq!(h.state.actions, vec![0]);

        // An overflow action keeps its index above the primary block: the
        // fourth pill on an opened rail is the first declared overflow action.
        h.press_toggle();
        assert_eq!(h.visible_actions(), 5);
        h.click_pill(3);
        assert_eq!(
            h.state.actions,
            vec![0, 3],
            "\"Archive\" is 3 = 3 primaries + 0"
        );
    }

    #[test]
    fn a_disabled_action_is_inert() {
        let mut h = Harness::new();
        h.settle();
        h.press_toggle();
        // "Delete" is the last pill, and disabled.
        h.click_pill(4);
        assert!(h.state.actions.is_empty());
    }

    #[test]
    fn collapse_on_action_shuts_the_group_behind_the_action_it_ran() {
        let mut h = Harness::new();
        h.state.collapse_on_action = true;
        h.settle();
        h.press_toggle();
        h.click_pill(3);
        h.settle();
        assert_eq!(h.state.actions, vec![3]);
        assert_eq!(h.state.expansions, vec![true, false]);
    }

    #[test]
    fn a_press_that_releases_off_its_action_reports_nothing() {
        let mut h = Harness::new();
        h.settle();
        let pills = h.pills();
        let (first, second) = (pills[0].center(), pills[1].center());
        h.event(pointer(PointerPhase::Down, first.x, first.y));
        h.event(pointer(PointerPhase::Up, second.x, second.y));
        assert!(h.state.actions.is_empty());
    }

    // ---- Motion and paint --------------------------------------------------

    #[test]
    fn the_overflow_items_arrive_staggered_and_reduce_motion_lands_them_together() {
        // A reveal reaches a box through the layout that follows the paint
        // which computed it, so a mid-run read has to be a whole frame.
        let mut h = Harness::new();
        h.settle();
        let at = h.toggle_point();
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        h.step(0.0);
        h.step(OVERFLOW_STAGGER.as_secs_f64() * 1_000.0 * 0.5);
        let mid = h.step_read(1.0);
        assert_eq!(
            mid.layers.len(),
            1,
            "only the leading overflow item has started: {:?}",
            mid.layers
        );

        let mut reduced = Harness::themed(reduced());
        reduced.settle();
        let at = reduced.toggle_point();
        reduced.event(pointer(PointerPhase::Down, at.x, at.y));
        reduced.event(pointer(PointerPhase::Up, at.x, at.y));
        reduced.step(0.0);
        reduced.step(OVERFLOW_STAGGER.as_secs_f64() * 1_000.0 * 0.5);
        let flat = reduced.step_read(1.0);
        assert_eq!(flat.layers.len(), 2, "both items are moving at once");
        assert!(flat.layers.iter().all(|alpha| *alpha == flat.layers[0]));
    }

    #[test]
    fn the_rail_paints_its_track_its_pills_and_its_toggle_mark() {
        let mut h = Harness::new();
        h.settle();
        let rec = h.read_after(0.0);
        assert!(!rec.shadows.is_empty(), "the track casts the panel shadow");
        assert!(!rec.clips.is_empty(), "`overflow-hidden`");
        // Three dots on the collapsed toggle, drawn as rounded rects.
        assert!(
            rec.rrects.len() >= 1 + 3 + 1 + 3,
            "track, three pills, the toggle and its three dots: {}",
            rec.rrects.len()
        );
        assert_eq!(rec.inks.len(), 3, "one run per visible action label");
    }

    // ---- Typeface: the action labels follow the live theme ------------------

    use crate::text::typeface_probe::{
        Face, Probe, assert_all, assert_control, assert_follows_a_live_family_swap,
    };

    /// Two primary actions with the overflow group expanded onto the rail.
    fn probe_view(_: &mut ()) -> frust::StackView<()> {
        frust::Stack(vec![any(overflow_actions::<(), _>(
            vec![overflow_action("Edit"), overflow_action("Share")],
            vec![overflow_action("Archive")],
            |_: &mut (), _| {},
        )
        .expanded(true))])
    }

    #[test]
    fn action_labels_paint_in_geist_under_the_beui_theme() {
        assert_control("the action labels", WINDOW);
        let mut probe = Probe::new(probe_view, WINDOW, crate::theme());
        // The reveal reaches the overflow action's box on the layout after the
        // paint that settles it, so the second frame is the settled rail.
        probe.frame();
        let runs = probe.frame();
        assert_all(
            "the action labels",
            "under the beUI theme",
            &runs,
            Face::Geist,
        );
        assert_eq!(
            runs.len(),
            3,
            "two primary labels and the revealed overflow one"
        );
    }

    #[test]
    fn action_labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the action labels", probe_view, WINDOW);
    }
}
