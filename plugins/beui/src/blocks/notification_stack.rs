//! Ports beUI's `notification-stack` composed block —
//! `components/motion/notification-stack.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `notification-stack`: *"Compact notification cards that spring from a
//! stacked summary into a readable list on hover, focus or tap."*
//!
//! | upstream | here |
//! |---|---|
//! | `STACK_PEEK = 8`, `STACK_INSET = 12` | [`toast_stack_peek`], [`toast_stack_inset`] — the same two numbers, shared |
//! | `maxVisible = 3`, `items.slice(0, max(1, maxVisible))` | [`NOTIFICATION_STACK_MAX_VISIBLE`] |
//! | root `w-full max-w-[22rem] rounded-3xl` | [`NOTIFICATION_STACK_MAX_WIDTH`], [`NOTIFICATION_STACK_RADIUS`] |
//! | inner `p-3`, background `absolute inset-0 rounded-3xl bg-muted` | [`NOTIFICATION_STACK_PADDING`], the resolved muted wash |
//! | card `rounded-2xl border border-border/60 bg-background px-4` | [`NOTIFICATION_STACK_CARD_RADIUS`], [`NOTIFICATION_STACK_BORDER_ALPHA`], [`NOTIFICATION_STACK_CARD_PADDING_X`] |
//! | card content `flex-col gap-1.5 py-4` | [`NOTIFICATION_STACK_CARD_PADDING_Y`], [`NOTIFICATION_STACK_TITLE_GAP`] |
//! | title `text-sm font-medium`, description/trailing `text-xs` | [`crate::style::TEXT_SM`] / [`crate::style::TEXT_XS`] |
//! | stack `grid gap-1`, collapsed `pb-2` | [`NOTIFICATION_STACK_GRID_GAP`], [`NOTIFICATION_STACK_COLLAPSED_PAD`] |
//! | footer `mt-2 min-h-9 gap-2 px-1` | [`NOTIFICATION_STACK_FOOTER_GAP`], [`NOTIFICATION_STACK_FOOTER_HEIGHT`], [`NOTIFICATION_STACK_FOOTER_INSET`] |
//! | count `size-7 rounded-full bg-orange-600 text-white` | [`NOTIFICATION_STACK_COUNT_BOX`] over the catalog's `--warning` |
//! | card transition `{duration: 0.32, ease: EASE_OUT}` | [`NOTIFICATION_STACK_CARD_RAMP`] |
//! | background transition `{duration: 0.26, ease: EASE_OUT}` | [`NOTIFICATION_STACK_BACKGROUND_RAMP`] |
//! | footer/layout transition `SPRING_LAYOUT` | [`NOTIFICATION_STACK_LAYOUT_RAMP`] |
//! | `ActionSwapText animation="roll"` label swap | [`NOTIFICATION_STACK_ROLL_TRAVEL`], [`NOTIFICATION_STACK_ROLL_EXIT`] |
//! | empty state `rounded-3xl bg-muted/70 px-5 py-8` + `BellOff` | [`NOTIFICATION_STACK_EMPTY_PADDING_X`], [`NOTIFICATION_STACK_EMPTY_PADDING_Y`], [`NOTIFICATION_STACK_EMPTY_WASH`] |
//!
//! # Premise corrections: no per-card dismiss, and no grouping
//!
//! The porting card describes "grouped notification cards … per-card dismiss
//! with neighbors re-springing, group-by-source if upstream does".
//! `notification-stack.tsx` has **neither**. It ships:
//!
//! - one `<motion.button>` wrapping the whole stack — there is no per-card
//!   control at all, no close affordance, and no `onDismiss` prop;
//! - a flat `items` array with no source, category or group field, and no
//!   grouping anywhere in the file;
//! - exactly two states, collapsed and expanded, plus an optional `onViewAll`
//!   that a second press follows instead of collapsing.
//!
//! So this port is the expand/collapse stack upstream actually ships. The
//! dismiss choreography the card describes *does* exist in this catalog — in
//! [`crate::components::animated_toast_stack`], whose own module docs record the
//! mirror-image correction (its card asked for a depth stack its source does not
//! have, and it borrowed **these** peek/inset numbers to build one). The two
//! files therefore meet in the middle, and this one imports
//! [`toast_stack_peek`]/[`toast_stack_inset`] rather than restating `STACK_PEEK`
//! and `STACK_INSET`, which are `notification-stack.tsx`'s to begin with.
//!
//! # The one real divergence: the box grows instead of overflowing
//!
//! Upstream's visible stack is `absolute inset-x-0 bottom-0`, so the button's
//! own box stays the *collapsed* footprint (an invisible sizer card plus the
//! footer) and the expanded list overflows **upward** out of it. That works on
//! the web because the browser hit-tests overflowing descendants, so the
//! expanded cards still count as "inside the button" for `pointerleave`.
//!
//! frust hit-tests a widget's layout box and nothing else, so an overflowing
//! stack would collapse the instant the pointer moved onto it — the interaction
//! would be unusable. This port therefore **animates its own height**: the box
//! grows downward from a fixed top edge as the stack spreads, so every card
//! stays hit-testable and nothing is painted outside the widget. What that
//! costs, and is worth naming: the expanded stack displaces whatever is below it
//! instead of covering whatever is above it, and the footer travels down rather
//! than staying put. The choreography — order, peek, inset, ramps — is
//! unchanged.
//!
//! # Controlled, with the collapse reported one pass late
//!
//! `expanded` is a prop and the widget never writes it: hover, focus, `Escape`
//! and a press all *report* the requested state through `on_expanded_change`.
//! One of those routes has no `EventCtx` to report from — the authoritative
//! hover read is `PaintCtx::is_hovered`, and a paint pass fires no callbacks —
//! so a collapse noticed at paint is recorded and drained on the next event
//! pass, the same deferral [`crate::blocks::expandable_action_bar`] makes for
//! its own `collapseDelay` and [`crate::motion::Presence::take_exited`] makes
//! for a finished exit.
//!
//! # Degradations against the web original
//!
//! - **No pointer-type branch, so no tap-dismisser.** Upstream reads
//!   `gesture.pointerType` to tell a finger from a mouse: a *touch* expansion
//!   arms an outside-tap dismisser, because a finger never "leaves". frust's
//!   `PointerEvent` carries no pointer type (the gap
//!   [`crate::components::context_menu`] records for its long-press), so both
//!   gestures take the mouse path: press to expand, press again to follow
//!   `on_view_all` or collapse. A touch user is never stranded — the second tap
//!   still closes it — they simply cannot close it by tapping elsewhere.
//! - **Deep cards pop rather than fade in.** Upstream toggles a plain
//!   `invisible` class on the non-primary cards' content, with no transition, so
//!   their text appears the instant `isExpanded` flips. That is ported as-is: it
//!   is a hard cut upstream and it is a hard cut here.
//! - **The clip inset is a width, not a `clipPath`.** Upstream insets each card
//!   with `clipPath: inset(0px Npx round 16px)`; a rounded-rect clip that both
//!   insets and re-rounds is expressible here only as a narrower rounded rect,
//!   which is what the deeper cards are drawn as. Visually identical for a
//!   two-sided inset; it would differ if upstream ever animated the round.
//! - **The count badge is a token, not `orange-600`.** Upstream hardcodes
//!   Tailwind's `bg-orange-600 dark:bg-orange-500`; the catalog resolves colour
//!   from tokens only, so the badge takes [`BeuiTokens`]' authored `--warning`
//!   — the same substitution [`crate::components::animated_badge`] makes for
//!   `amber-500`. Its `shadow-[inset_…]` ring has no inset-shadow primitive
//!   here and is dropped.
//! - **Text, not nodes.** `title`/`description`/`trailing` are `ReactNode`
//!   upstream and strings here, single-line rather than `line-clamp`ed — the
//!   narrowing every text-bearing port in this catalog records.
//! - **The label swap is a roll, re-derived.** Upstream composes
//!   `ActionSwapText animation="roll"`; that is a whole `View`/`Widget` pair in
//!   this crate, not a paint helper, so the footer re-derives the same numbers
//!   ([`NOTIFICATION_STACK_ROLL_TRAVEL`], [`NOTIFICATION_STACK_ROLL_EXIT`],
//!   [`NOTIFICATION_STACK_ROLL_ENTER`]) from the same source rather than nesting
//!   a button inside a button. No arrow glyph accompanies the expanded label,
//!   for the same no-icon-set reason the empty state's bell is drawn.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    ErasedCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size,
    View, Widget, erase_callback, erase_callback_arg,
    text::{FontWeight, TextStyle},
};
use frust::{FrameTime, Theme};

use crate::components::animated_toast_stack::{toast_stack_inset, toast_stack_peek};
use crate::motion::Ramp;
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::BeuiTokens;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT, SPRING_SWAP};

// ---- Metrics ---------------------------------------------------------------

/// How many cards the stack shows, at most (`maxVisible = 3`). The floor of one
/// is upstream's own `Math.max(1, maxVisible)`.
pub const NOTIFICATION_STACK_MAX_VISIBLE: usize = 3;

/// The stack's width ceiling, in logical px (`max-w-[22rem]`).
pub const NOTIFICATION_STACK_MAX_WIDTH: f64 = 352.0;

/// The stack's outer radius, in logical px (`rounded-3xl`).
pub const NOTIFICATION_STACK_RADIUS: f64 = style::RADIUS_3XL;

/// The inner padding around the cards and footer, in logical px (`p-3`).
pub const NOTIFICATION_STACK_PADDING: f64 = 12.0;

/// A card's radius, in logical px (`rounded-2xl`).
pub const NOTIFICATION_STACK_CARD_RADIUS: f64 = style::RADIUS_2XL;

/// A card's horizontal padding, in logical px (`px-4`).
pub const NOTIFICATION_STACK_CARD_PADDING_X: f64 = 16.0;

/// A card's vertical padding, in logical px (`py-4`).
pub const NOTIFICATION_STACK_CARD_PADDING_Y: f64 = 16.0;

/// The gap between a card's title row and its description, in logical px
/// (`gap-1.5`).
pub const NOTIFICATION_STACK_TITLE_GAP: f64 = 6.0;

/// The gap between two expanded cards, in logical px (`grid gap-1`).
pub const NOTIFICATION_STACK_GRID_GAP: f64 = 4.0;

/// The extra room under the collapsed stack, in logical px (`pb-2`) — where the
/// peeking cards show.
pub const NOTIFICATION_STACK_COLLAPSED_PAD: f64 = 8.0;

/// The gap between the stack and the footer, in logical px (`mt-2`).
pub const NOTIFICATION_STACK_FOOTER_GAP: f64 = 8.0;

/// The footer's height, in logical px (`min-h-9`).
pub const NOTIFICATION_STACK_FOOTER_HEIGHT: f64 = 36.0;

/// The footer's own horizontal inset, in logical px (`px-1`).
pub const NOTIFICATION_STACK_FOOTER_INSET: f64 = 4.0;

/// The gap between the count badge and the footer label, in logical px
/// (`gap-2`).
pub const NOTIFICATION_STACK_COUNT_GAP: f64 = 8.0;

/// The count badge's box, in logical px (`size-7`).
pub const NOTIFICATION_STACK_COUNT_BOX: f64 = 28.0;

/// A card's hairline alpha (`border-border/60`).
pub const NOTIFICATION_STACK_BORDER_ALPHA: f32 = 0.6;

/// The empty state's horizontal padding, in logical px (`px-5`).
pub const NOTIFICATION_STACK_EMPTY_PADDING_X: f64 = 20.0;

/// The empty state's vertical padding, in logical px (`py-8`).
pub const NOTIFICATION_STACK_EMPTY_PADDING_Y: f64 = 32.0;

/// The empty state's surface wash (`bg-muted/70`).
pub const NOTIFICATION_STACK_EMPTY_WASH: f32 = 0.7;

/// The empty state's mark, in logical px (`h-4 w-4`).
pub const NOTIFICATION_STACK_EMPTY_MARK: f64 = 16.0;

/// The gap between that mark and the empty label, in logical px (`gap-2`).
pub const NOTIFICATION_STACK_EMPTY_GAP: f64 = 8.0;

// ---- Motion ----------------------------------------------------------------

/// The per-card transition: `{ duration: 0.32, ease: EASE_OUT }` — the peek and
/// the clip inset both run on it.
pub const NOTIFICATION_STACK_CARD_RAMP: Ramp = Ramp::eased(Duration::from_millis(320), EASE_OUT);

/// The muted background's own transition: `{ duration: 0.26, ease: EASE_OUT }`,
/// deliberately shorter than the cards' so the surface arrives first.
pub const NOTIFICATION_STACK_BACKGROUND_RAMP: Ramp =
    Ramp::eased(Duration::from_millis(260), EASE_OUT);

/// The layout transition the footer and the box height travel on
/// (`SPRING_LAYOUT`).
pub const NOTIFICATION_STACK_LAYOUT_RAMP: Ramp = Ramp::spring(SPRING_LAYOUT);

/// How far a rolling footer label travels, as a fraction of its own height
/// (`y: "90%"` — `action-swap.tsx`'s roll, the treatment upstream names).
pub const NOTIFICATION_STACK_ROLL_TRAVEL: f64 = 0.9;

/// The rolling label's exit: `ROLL_EXIT_TRANSITION = { duration: 0.14, ease:
/// EASE_OUT }`.
pub const NOTIFICATION_STACK_ROLL_EXIT: Ramp = Ramp::eased(Duration::from_millis(140), EASE_OUT);

/// The rolling label's entrance: `SPRING_SWAP`, `action-swap.tsx`'s own arriving
/// transition for the roll.
pub const NOTIFICATION_STACK_ROLL_ENTER: Ramp = Ramp::spring(SPRING_SWAP);

// ---- The view --------------------------------------------------------------

/// One notification: its identity, its title, and the two optional notes
/// upstream renders beside and under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationStackItem {
    id: String,
    title: String,
    description: Option<String>,
    trailing: Option<String>,
}

/// A notification identified by `id` and headed `title`.
pub fn notification(id: impl Into<String>, title: impl Into<String>) -> NotificationStackItem {
    NotificationStackItem {
        id: id.into(),
        title: title.into(),
        description: None,
        trailing: None,
    }
}

impl NotificationStackItem {
    /// The secondary line under the title (`item.description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The note on the title's own row, right-aligned (`item.trailing` — a
    /// timestamp, in upstream's own example).
    pub fn trailing(mut self, trailing: impl Into<String>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// This notification's id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A view-held expansion callback, erased on build.
type OnExpandedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A view-held "view all" callback, erased on build.
type OnViewAll<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI notification stack. See the [module docs](self).
pub struct NotificationStackView<State: 'static> {
    items: Vec<NotificationStackItem>,
    expanded: bool,
    max_visible: usize,
    collapsed_label: String,
    expanded_label: String,
    empty_label: String,
    on_expanded_change: OnExpandedChange<State>,
    on_view_all: Option<OnViewAll<State>>,
}

/// Create a notification stack showing `items`, spread while `expanded`, and
/// reporting a requested spread through `on_expanded_change` — a **controlled**
/// component.
pub fn notification_stack<State: 'static, F: Fn(&mut State, bool) + 'static>(
    items: Vec<NotificationStackItem>,
    expanded: bool,
    on_expanded_change: F,
) -> NotificationStackView<State> {
    NotificationStackView {
        items,
        expanded,
        max_visible: NOTIFICATION_STACK_MAX_VISIBLE,
        collapsed_label: "Notifications".to_string(),
        expanded_label: "View all".to_string(),
        empty_label: "All caught up".to_string(),
        on_expanded_change: Rc::new(on_expanded_change),
        on_view_all: None,
    }
}

impl<State: 'static> NotificationStackView<State> {
    /// Follow a second press on the expanded stack instead of collapsing it
    /// (`onViewAll`).
    pub fn on_view_all<F: Fn(&mut State) + 'static>(mut self, on_view_all: F) -> Self {
        self.on_view_all = Some(Rc::new(on_view_all));
        self
    }

    /// How many cards to show (`maxVisible`, floored at one).
    pub fn max_visible(mut self, max_visible: usize) -> Self {
        self.max_visible = max_visible.max(1);
        self
    }

    /// The footer label while collapsed (`collapsedLabel`).
    pub fn collapsed_label(mut self, label: impl Into<String>) -> Self {
        self.collapsed_label = label.into();
        self
    }

    /// The footer label while expanded (`expandedLabel`).
    pub fn expanded_label(mut self, label: impl Into<String>) -> Self {
        self.expanded_label = label.into();
        self
    }

    /// The label shown when there is nothing to show (`emptyLabel`).
    pub fn empty_label(mut self, label: impl Into<String>) -> Self {
        self.empty_label = label.into();
        self
    }

    /// The cards the stack shows (`items.slice(0, max(1, maxVisible))`).
    fn visible_items(&self) -> &[NotificationStackItem] {
        let count = self.items.len().min(self.max_visible.max(1));
        &self.items[..count]
    }
}

/// The resolved stack palette.
struct NotificationStackColors {
    /// The tray behind the cards (`bg-muted`).
    tray: Color,
    /// A card's own fill (`bg-background`).
    card: Color,
    /// A card's hairline (`border-border`, washed to 60% at the point of use).
    border: Color,
    /// Title ink (`text-foreground`).
    ink: Color,
    /// Description and trailing ink (`text-muted-foreground`).
    dim_ink: Color,
    /// The count badge (`bg-orange-600` → the catalog's `--warning`).
    count: Color,
    /// Ink on that badge (`text-white`).
    on_count: Color,
}

/// Resolve the palette, falling back to the vendored **light** table with no
/// theme threaded — the unthemed posture every component in this catalog takes.
fn resolve_colors(theme: Option<&Theme>) -> NotificationStackColors {
    let tokens = BeuiTokens::resolve(theme);
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            NotificationStackColors {
                tray: s.surface_container,
                card: s.surface,
                border: s.outline_variant,
                ink: s.on_surface,
                dim_ink: s.on_surface_variant,
                count: tokens.warning,
                on_count: s.on_primary,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            NotificationStackColors {
                tray: p.muted,
                card: p.background,
                border: p.border,
                ink: p.foreground,
                dim_ink: p.muted_foreground,
                count: tokens.warning,
                on_count: p.primary_foreground,
            }
        }
    }
}

/// The card title's style (`text-sm font-medium`), in the theme's
/// `label_large` family.
fn title_style(theme: Option<&Theme>) -> TextStyle {
    themed_style(
        crate::text::label_style(style::TEXT_SM),
        ThemeTextType::LabelLarge,
        theme,
    )
}

/// The description/trailing style (`text-xs`, regular weight), in
/// [`title_style`]'s family.
fn body_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        size: style::TEXT_XS as f32,
        weight: FontWeight::REGULAR,
        ..title_style(theme)
    }
}

/// The count badge's style (`text-xs font-medium`), in [`title_style`]'s
/// family.
fn count_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        size: style::TEXT_XS as f32,
        ..title_style(theme)
    }
}

/// One retained card.
struct CardRow {
    id: String,
    title: LabelRun,
    description: Option<LabelRun>,
    trailing: Option<LabelRun>,
    /// The card's peek past the one in front (`animate.y`), on the card ramp.
    peek: Lane,
    /// The card's per-side clip inset (`animate.clipPath`), on the card ramp.
    inset: Lane,
    /// The card's expanded stacking offset (`layout="position"`), on
    /// `SPRING_LAYOUT`.
    place: Lane,
    /// The card's own height, from the last layout.
    height: f64,
}

impl CardRow {
    fn new(item: &NotificationStackItem, index: usize, expanded: bool) -> Self {
        CardRow {
            id: item.id.clone(),
            title: LabelRun::new(item.title.clone()),
            description: item.description.as_ref().map(LabelRun::new),
            trailing: item.trailing.as_ref().map(LabelRun::new),
            peek: Lane::at_rest(
                NOTIFICATION_STACK_CARD_RAMP,
                if expanded {
                    0.0
                } else {
                    toast_stack_peek(index)
                },
            ),
            inset: Lane::at_rest(
                NOTIFICATION_STACK_CARD_RAMP,
                if expanded {
                    0.0
                } else {
                    collapsed_inset(index)
                },
            ),
            place: Lane::at_rest(NOTIFICATION_STACK_LAYOUT_RAMP, 0.0),
            height: 0.0,
        }
    }

    /// Adopt `item` in place, reporting whether a re-measure is owed.
    fn sync(&mut self, item: &NotificationStackItem) -> bool {
        let mut changed = self.id != item.id;
        self.id = item.id.clone();
        changed |= self.title.set_content(item.title.clone());
        changed |= sync_run(&mut self.description, item.description.as_ref());
        changed |= sync_run(&mut self.trailing, item.trailing.as_ref());
        changed
    }
}

/// The per-side inset a depth-`index` card is clipped by while collapsed —
/// `inset(0px index * STACK_INSET)`, the width form of upstream's `clipPath`.
///
/// Unbounded here and clamped against the actual card width at the point of
/// use, which is what [`toast_stack_inset`] does with the width it is given.
fn collapsed_inset(index: usize) -> f64 {
    // `toast_stack_inset` is the same `index * STACK_INSET`, clamped to half the
    // width; the width is not known at construction, so the clamp waits.
    toast_stack_inset(index, f64::INFINITY)
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

impl<State: 'static> View<State> for NotificationStackView<State> {
    type Element = NotificationStackWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> NotificationStackWidget {
        let spread = if self.expanded { 1.0 } else { 0.0 };
        NotificationStackWidget {
            rows: self
                .visible_items()
                .iter()
                .enumerate()
                .map(|(index, item)| CardRow::new(item, index, self.expanded))
                .collect(),
            total: self.items.len(),
            expanded: self.expanded,
            collapsed_label: LabelRun::new(self.collapsed_label.clone()),
            expanded_label: LabelRun::new(self.expanded_label.clone()),
            empty_label: LabelRun::new(self.empty_label.clone()),
            count: LabelRun::new(self.items.len().to_string()),
            spread: Lane::at_rest(NOTIFICATION_STACK_LAYOUT_RAMP, spread),
            background: Lane::at_rest(NOTIFICATION_STACK_BACKGROUND_RAMP, spread),
            roll: Lane::at_rest(NOTIFICATION_STACK_ROLL_ENTER, spread),
            width: 0.0,
            collapsed_height: 0.0,
            expanded_height: 0.0,
            hovered: false,
            focused: false,
            armed: false,
            pending_expanded: None,
            on_expanded_change: erase_callback_arg(&self.on_expanded_change),
            on_view_all: self.on_view_all.as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut NotificationStackWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_expanded_change = erase_callback_arg(&self.on_expanded_change);
        element.on_view_all = self.on_view_all.as_ref().map(erase_callback);
        let mut flags = ChangeFlags::NONE;

        let visible = self.visible_items();
        if element.rows.len() != visible.len() {
            element.rows = visible
                .iter()
                .enumerate()
                .map(|(index, item)| CardRow::new(item, index, self.expanded))
                .collect();
            element.armed = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (row, item) in element.rows.iter_mut().zip(visible.iter()) {
                if row.sync(item) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        if element.total != self.items.len() {
            element.total = self.items.len();
            element.count.set_content(self.items.len().to_string());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let labels_changed = element
            .collapsed_label
            .set_content(self.collapsed_label.clone())
            | element
                .expanded_label
                .set_content(self.expanded_label.clone())
            | element.empty_label.set_content(self.empty_label.clone());
        if labels_changed {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.expanded != self.expanded {
            element.expanded = self.expanded;
            element.retarget_spread();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`NotificationStackView`].
pub struct NotificationStackWidget {
    rows: Vec<CardRow>,
    /// Every notification, not just the visible ones — the count badge and the
    /// accessible name both report the whole queue.
    total: usize,
    /// The app-confirmed spread.
    expanded: bool,
    collapsed_label: LabelRun,
    expanded_label: LabelRun,
    empty_label: LabelRun,
    count: LabelRun,
    /// The `0 → 1` spread the box height and the footer travel on.
    spread: Lane,
    /// The muted tray's own, shorter spread.
    background: Lane,
    /// The footer label's roll, `0` collapsed and `1` expanded.
    roll: Lane,
    /// The stack's resolved width, from the last layout.
    width: f64,
    /// The collapsed box height, from the last layout.
    collapsed_height: f64,
    /// The expanded box height, from the last layout.
    expanded_height: f64,
    /// The latched hover, self-corrected from `PaintCtx::is_hovered`.
    hovered: bool,
    /// Whether keyboard focus is on the stack — upstream's `hasFocus` ref, which
    /// is what keeps a focused stack open under a leaving pointer.
    focused: bool,
    /// Whether a `Down` armed the stack.
    armed: bool,
    /// A spread change noticed at paint, drained on the next event pass.
    pending_expanded: Option<bool>,
    on_expanded_change: ErasedArgCallback<bool>,
    on_view_all: Option<ErasedCallback>,
}

impl NotificationStackWidget {
    /// Whether there is anything to show (`primaryItem`).
    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Re-aim every lane at the spread the confirmed flag calls for.
    fn retarget_spread(&mut self) {
        let target = if self.expanded { 1.0 } else { 0.0 };
        self.spread.retarget(target);
        self.background.retarget(target);
        // The roll is asymmetric, as `action-swap.tsx` authors it: a spring in,
        // a 140ms tween out.
        let ramp = if self.expanded {
            NOTIFICATION_STACK_ROLL_ENTER
        } else {
            NOTIFICATION_STACK_ROLL_EXIT
        };
        self.roll.retarget_with(ramp, target);
        let expanded = self.expanded;
        for (index, row) in self.rows.iter_mut().enumerate() {
            row.peek.retarget(if expanded {
                0.0
            } else {
                toast_stack_peek(index)
            });
            row.inset.retarget(if expanded {
                0.0
            } else {
                collapsed_inset(index)
            });
        }
    }

    /// Re-aim each card's expanded stacking offset — the running sum of the
    /// cards above it. Called from `layout`, which knows the heights.
    fn retarget_places(&mut self) {
        let expanded = self.expanded;
        let mut running = 0.0;
        for row in &mut self.rows {
            row.place.retarget(if expanded { running } else { 0.0 });
            running += row.height + NOTIFICATION_STACK_GRID_GAP;
        }
    }

    /// The width inside the tray's own padding.
    fn inner_width(&self) -> f64 {
        (self.width - NOTIFICATION_STACK_PADDING * 2.0).max(0.0)
    }

    /// The stack band's collapsed height: the front card plus the room the
    /// peeking cards show in (`pb-2`).
    fn collapsed_stack_height(&self) -> f64 {
        self.rows
            .first()
            .map_or(0.0, |row| row.height + NOTIFICATION_STACK_COLLAPSED_PAD)
    }

    /// The stack band's expanded height: every card, gapped.
    fn expanded_stack_height(&self) -> f64 {
        let cards: f64 = self.rows.iter().map(|row| row.height).sum();
        cards + NOTIFICATION_STACK_GRID_GAP * self.rows.len().saturating_sub(1) as f64
    }

    /// Card `index`'s box in widget-local coordinates, at the spread the lanes
    /// currently show.
    fn card_rect(&self, index: usize) -> Option<Rect> {
        let row = self.rows.get(index)?;
        let inner = self.inner_width();
        let inset = row.inset.value().clamp(0.0, (inner / 2.0).max(0.0));
        Some(Rect::from_origin_size(
            Point::new(
                NOTIFICATION_STACK_PADDING + inset,
                NOTIFICATION_STACK_PADDING + row.place.value() + row.peek.value(),
            ),
            Size::new((inner - 2.0 * inset).max(0.0), row.height),
        ))
    }

    /// The footer band's box, under the currently-displayed stack.
    fn footer_rect(&self) -> Rect {
        let spread = self.spread.value().clamp(0.0, 1.0);
        let collapsed = self.collapsed_stack_height();
        let stack = collapsed + (self.expanded_stack_height() - collapsed) * spread;
        Rect::from_origin_size(
            Point::new(
                NOTIFICATION_STACK_PADDING + NOTIFICATION_STACK_FOOTER_INSET,
                NOTIFICATION_STACK_PADDING + stack + NOTIFICATION_STACK_FOOTER_GAP,
            ),
            Size::new(
                (self.inner_width() - NOTIFICATION_STACK_FOOTER_INSET * 2.0).max(0.0),
                NOTIFICATION_STACK_FOOTER_HEIGHT,
            ),
        )
    }

    /// The whole box's currently-displayed height.
    fn displayed_height(&self) -> f64 {
        let spread = self.spread.value().clamp(0.0, 1.0);
        self.collapsed_height + (self.expanded_height - self.collapsed_height) * spread
    }

    /// Whether card `index`'s content is drawn: the front card always, a deeper
    /// one only once expanded (upstream's plain `invisible` class — a hard cut,
    /// not a fade).
    fn content_visible(&self, index: usize) -> bool {
        index == 0 || self.expanded
    }

    /// Ask the owner for a spread, from an event pass.
    fn request_expanded(&mut self, ctx: &mut EventCtx, expanded: bool) {
        if self.expanded == expanded {
            return;
        }
        (self.on_expanded_change)(ctx, expanded);
    }

    /// Advance every lane to `now`, returning whether anything is still moving.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.spread.snap();
            self.background.snap();
            self.roll.snap();
            for row in &mut self.rows {
                row.peek.snap();
                row.inset.snap();
                row.place.snap();
            }
        }
        let mut moving = self.spread.advance(now);
        moving |= self.background.advance(now);
        moving |= self.roll.advance(now);
        for row in &mut self.rows {
            moving |= row.peek.advance(now);
            moving |= row.inset.advance(now);
            moving |= row.place.advance(now);
        }
        moving
    }

    /// Paint card `index`'s surface, hairline and (when visible) its text.
    fn paint_card(
        &self,
        origin: Point,
        scene: &mut dyn PaintScene,
        colors: &NotificationStackColors,
        index: usize,
    ) {
        let Some(rect) = self.card_rect(index) else {
            return;
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let at = origin + rect.origin().to_vec2();
        let radius =
            style::resolve_radius(NOTIFICATION_STACK_CARD_RADIUS, rect.width(), rect.height());
        scene.fill_rounded_rect(at, rect.size(), radius, colors.card);
        paint_hairline(
            scene,
            at,
            rect.size(),
            radius,
            style::with_alpha(colors.border, NOTIFICATION_STACK_BORDER_ALPHA),
        );

        if !self.content_visible(index) {
            return;
        }
        let row = &self.rows[index];
        let text_x = at.x + NOTIFICATION_STACK_CARD_PADDING_X;
        let text_right = at.x + rect.width() - NOTIFICATION_STACK_CARD_PADDING_X;
        let mut y = at.y + NOTIFICATION_STACK_CARD_PADDING_Y;

        // The title row: title left, trailing note right (`justify-between`).
        let title_size = row.title.size();
        row.title.paint(Point::new(text_x, y), colors.ink, scene);
        if let Some(trailing) = &row.trailing {
            let size = trailing.size();
            trailing.paint(
                Point::new((text_right - size.width).max(text_x), y),
                colors.dim_ink,
                scene,
            );
        }
        y += title_size.height;

        if let Some(description) = &row.description {
            y += NOTIFICATION_STACK_TITLE_GAP;
            description.paint(Point::new(text_x, y), colors.dim_ink, scene);
        }
    }

    /// Paint the footer: the count badge and the rolling label.
    fn paint_footer(
        &self,
        origin: Point,
        scene: &mut dyn PaintScene,
        colors: &NotificationStackColors,
    ) {
        let rect = self.footer_rect();
        let at = origin + rect.origin().to_vec2();
        let badge = Point::new(
            at.x,
            at.y + (rect.height() - NOTIFICATION_STACK_COUNT_BOX) / 2.0,
        );
        let badge_size = Size::new(NOTIFICATION_STACK_COUNT_BOX, NOTIFICATION_STACK_COUNT_BOX);
        scene.fill_rounded_rect(
            badge,
            badge_size,
            NOTIFICATION_STACK_COUNT_BOX / 2.0,
            colors.count,
        );
        let count = self.count.size();
        self.count.paint(
            Point::new(
                badge.x + (NOTIFICATION_STACK_COUNT_BOX - count.width) / 2.0,
                badge.y + (NOTIFICATION_STACK_COUNT_BOX - count.height) / 2.0,
            ),
            colors.on_count,
            scene,
        );

        // The rolling label swap: the arriving label rises into place while the
        // leaving one rolls out, each travelling 90% of its own height, both
        // clipped to the footer band (`overflow-hidden`).
        let roll = self.roll.value().clamp(0.0, 1.0);
        let label_x = at.x + NOTIFICATION_STACK_COUNT_BOX + NOTIFICATION_STACK_COUNT_GAP;
        let band = Size::new((rect.width() - (label_x - at.x)).max(0.0), rect.height());
        scene.push_clip(Point::new(label_x, at.y), band);
        for (run, share, direction) in [
            (&self.collapsed_label, 1.0 - roll, -1.0),
            (&self.expanded_label, roll, 1.0),
        ] {
            if share <= 0.0 {
                continue;
            }
            let size = run.size();
            let travel = (1.0 - share) * size.height * NOTIFICATION_STACK_ROLL_TRAVEL * direction;
            run.paint(
                Point::new(label_x, at.y + (rect.height() - size.height) / 2.0 + travel),
                style::with_alpha(colors.ink, share as f32),
                scene,
            );
        }
        scene.pop_clip();
    }

    /// Paint the empty state: a washed tray, a struck-through bell, and the
    /// empty label.
    fn paint_empty(
        &self,
        origin: Point,
        size: Size,
        scene: &mut dyn PaintScene,
        colors: &NotificationStackColors,
    ) {
        let radius = style::resolve_radius(NOTIFICATION_STACK_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(
            origin,
            size,
            radius,
            style::with_alpha(colors.tray, NOTIFICATION_STACK_EMPTY_WASH),
        );
        let label = self.empty_label.size();
        let content = NOTIFICATION_STACK_EMPTY_MARK + NOTIFICATION_STACK_EMPTY_GAP + label.width;
        let x = origin.x + ((size.width - content) / 2.0).max(NOTIFICATION_STACK_EMPTY_PADDING_X);
        draw_bell_off(
            scene,
            Point::new(
                x,
                origin.y + (size.height - NOTIFICATION_STACK_EMPTY_MARK) / 2.0,
            ),
            colors.dim_ink,
        );
        self.empty_label.paint(
            Point::new(
                x + NOTIFICATION_STACK_EMPTY_MARK + NOTIFICATION_STACK_EMPTY_GAP,
                origin.y + (size.height - label.height) / 2.0,
            ),
            colors.dim_ink,
            scene,
        );
    }

    /// The keyboard arm: `Escape` collapses, `Space`/`Enter` activates.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> EventResult {
        if matches!(key.key, Key::Named(NamedKey::Escape)) {
            if !self.expanded {
                return EventResult::Ignored;
            }
            // Focus stays where the keyboard put it: blurring here would
            // collapse the stack *and* throw the user back to the top of the
            // document, which is upstream's own recorded reason.
            self.request_expanded(ctx, false);
            return EventResult::Handled;
        }
        if is_activation_key(key) {
            self.focused = true;
            self.activate(ctx);
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    /// What an activation does: expand a collapsed stack, else follow
    /// `on_view_all` when there is one, else collapse.
    fn activate(&mut self, ctx: &mut EventCtx) {
        if !self.expanded {
            self.request_expanded(ctx, true);
        } else if let Some(on_view_all) = self.on_view_all.as_mut() {
            on_view_all(ctx);
        } else {
            self.request_expanded(ctx, false);
        }
    }
}

/// Stroke a 1px hairline just inside `size`, so it lands within the surface
/// rather than straddling its edge.
fn paint_hairline(scene: &mut dyn PaintScene, at: Point, size: Size, radius: f64, color: Color) {
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

/// Draw Lucide's `BellOff` in a [`NOTIFICATION_STACK_EMPTY_MARK`] box at
/// `origin`: a bell silhouette with a slash through it.
///
/// Drawn rather than shaped, the route [`crate::components::animated_badge`]
/// takes for the same reason — this catalog bundles no icon set.
fn draw_bell_off(scene: &mut dyn PaintScene, origin: Point, color: Color) {
    let m = NOTIFICATION_STACK_EMPTY_MARK;
    let brush = Brush::Solid(color);
    let stroke = 1.25;
    let dome = RoundedRect::new(m * 0.2, m * 0.15, m * 0.8, m * 0.65, m * 0.3);
    scene.stroke_path(
        origin,
        &Shape::to_path(&dome, style::PATH_TOLERANCE),
        stroke,
        &brush,
    );
    let clapper = RoundedRect::new(m * 0.4, m * 0.72, m * 0.6, m * 0.85, m * 0.07);
    scene.fill_path(
        origin,
        &Shape::to_path(&clapper, style::PATH_TOLERANCE),
        &brush,
    );
    let mut slash = BezPath::new();
    slash.move_to((m * 0.1, m * 0.1));
    slash.line_to((m * 0.9, m * 0.9));
    scene.stroke_path(origin, &slash, stroke, &brush);
}

impl Widget for NotificationStackWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let title = title_style(theme);
        let body = body_style(theme);
        let count = count_style(theme);

        self.width = NOTIFICATION_STACK_MAX_WIDTH.min(bc.max().width.max(0.0));

        if self.is_empty() {
            let label = self.empty_label.layout(ctx, &title);
            let height = label.height.max(NOTIFICATION_STACK_EMPTY_MARK)
                + NOTIFICATION_STACK_EMPTY_PADDING_Y * 2.0;
            self.collapsed_height = height;
            self.expanded_height = height;
            return bc.constrain(Size::new(self.width, height));
        }

        for row in &mut self.rows {
            let title_size = row.title.layout(ctx, &title);
            let trailing = row
                .trailing
                .as_mut()
                .map_or(0.0, |run| run.layout(ctx, &body).height);
            let description = row
                .description
                .as_mut()
                .map(|run| run.layout(ctx, &body).height);
            let head = title_size.height.max(trailing);
            row.height = NOTIFICATION_STACK_CARD_PADDING_Y * 2.0
                + head
                + description.map_or(0.0, |h| NOTIFICATION_STACK_TITLE_GAP + h);
        }
        self.count.layout(ctx, &count);
        self.collapsed_label.layout(ctx, &title);
        self.expanded_label.layout(ctx, &title);
        self.retarget_places();

        let chrome = NOTIFICATION_STACK_PADDING * 2.0
            + NOTIFICATION_STACK_FOOTER_GAP
            + NOTIFICATION_STACK_FOOTER_HEIGHT;
        self.collapsed_height = self.collapsed_stack_height() + chrome;
        self.expanded_height = self.expanded_stack_height() + chrome;

        bc.constrain(Size::new(self.width, self.displayed_height()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The authoritative hover read. A pointer that simply left the stack
        // produces no `Move`, so this is the only signal that it did — and a
        // paint pass cannot fire the owner's callback, so the collapse is
        // recorded for the next event pass.
        let hovered = ctx.is_hovered();
        if self.hovered != hovered {
            self.hovered = hovered;
            if !hovered && !self.focused && self.expanded {
                self.pending_expanded = Some(false);
            }
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let moving = self.advance(ctx.frame_time(), reduce_motion);
        let origin = ctx.origin();
        let size = ctx.size();

        if self.is_empty() {
            self.paint_empty(origin, size, scene, &colors);
            return;
        }

        // The muted tray, on its own shorter ramp than the cards.
        let tray_spread = self.background.value().clamp(0.0, 1.0);
        let tray_height =
            self.collapsed_height + (self.expanded_height - self.collapsed_height) * tray_spread;
        let tray_size = Size::new(self.width, tray_height.max(0.0));
        scene.fill_rounded_rect(
            origin,
            tray_size,
            style::resolve_radius(NOTIFICATION_STACK_RADIUS, tray_size.width, tray_size.height),
            colors.tray,
        );

        // Back to front, so the newest card lands on top of its neighbours
        // (`zIndex: visibleItems.length - index`).
        for index in (0..self.rows.len()).rev() {
            self.paint_card(origin, scene, &colors, index);
        }
        self.paint_footer(origin, scene, &colors);

        // The box height travels with the spread, so a moving stack asks for a
        // relayout rather than a bare repaint.
        if moving {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        // Drain the collapse the paint pass noticed — the deferral the module
        // docs describe.
        if let Some(expanded) = self.pending_expanded.take() {
            self.request_expanded(ctx, expanded);
        }
        if self.is_empty() {
            return EventResult::Ignored;
        }
        let size = Size::new(self.width, self.displayed_height());
        match event {
            InputEvent::Key(key) => self.handle_key(ctx, key),
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, size) {
                        return EventResult::Ignored;
                    }
                    self.armed = true;
                    self.focused = true;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.armed {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = inside(p.position, size);
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    // A hovering pointer expands; one that left collapses,
                    // unless the keyboard is holding it open.
                    if over {
                        self.request_expanded(ctx, true);
                    } else if !self.focused {
                        self.request_expanded(ctx, false);
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    if !self.armed {
                        return EventResult::Ignored;
                    }
                    self.armed = false;
                    if !inside(p.position, size) {
                        return EventResult::Handled;
                    }
                    // Upstream reads the spread from where the *gesture*
                    // started, so a browser that focuses on contact cannot make
                    // the first tap follow `onViewAll`. The equivalent here is
                    // that this widget never writes `expanded` itself, so the
                    // flag cannot have moved mid-press.
                    self.activate(ctx);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if !self.armed {
                        return EventResult::Ignored;
                    }
                    self.armed = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Upstream publishes exactly one node: the button, named by the count
        // and by what a press would do next. The cards are `span`s.
        let empty = self.is_empty();
        let expanded = self.expanded;
        let label = if empty {
            self.empty_label.content().to_string()
        } else if expanded {
            format!(
                "{} notifications. {}.",
                self.total,
                self.expanded_label.content()
            )
        } else {
            format!("{} notifications. Expand notifications.", self.total)
        };
        ctx.push_node(if empty { Role::Status } else { Role::Button }, |node| {
            node.set_label(label.as_str());
            if !empty {
                node.set_expanded(expanded);
                node.add_action(Action::Click);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, Modifiers, PointerButton, PointerEvent, SemanticsUpdate};
    use std::any::Any;

    /// Records what the widget paints.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        clips: Vec<(Point, Size)>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, b: &Brush) {
            if let Brush::Solid(color) = b {
                self.strokes.push(*color);
            }
        }
        fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {}
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_transform(&mut self, _t: Affine) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct App {
        expanded: Option<bool>,
        expanded_calls: u32,
        view_all_calls: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<NotificationStackItem> {
        vec![
            notification("a", "Sarah commented on your draft")
                .description("Left three notes on the pricing section")
                .trailing("2m"),
            notification("b", "Build #1841 passed").trailing("18m"),
            notification("c", "Weekly digest is ready")
                .description("Nine new mentions across four repositories"),
            notification("d", "This one is past maxVisible"),
        ]
    }

    fn view(expanded: bool) -> NotificationStackView<App> {
        notification_stack::<App, _>(items(), expanded, |s: &mut App, next: bool| {
            s.expanded = Some(next);
            s.expanded_calls += 1;
        })
    }

    fn view_with_all(expanded: bool) -> NotificationStackView<App> {
        view(expanded).on_view_all(|s: &mut App| s.view_all_calls += 1)
    }

    fn build_from(v: &NotificationStackView<App>) -> NotificationStackWidget {
        let mut counter = 0u64;
        View::<App>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut NotificationStackWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, 800.0)),
        )
    }

    fn laid_out(expanded: bool) -> (NotificationStackWidget, Size) {
        let mut w = build_from(&view(expanded));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(w: &mut NotificationStackWidget, size: Size, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn retarget(w: &mut NotificationStackWidget, from: bool, to: bool) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<App>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    /// Paint/relayout far past every ramp so the layout animation lands.
    fn settle(w: &mut NotificationStackWidget, size: Size, from_ms: f64) -> Size {
        let mut size = size;
        for step in 0..30 {
            paint_at(w, size, from_ms + step as f64 * 300.0);
            size = layout(w);
        }
        size
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn key_event(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(w: &mut NotificationStackWidget, size: Size, event: &InputEvent, state: &mut App) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The stack shows at most `maxVisible` cards even when the queue is
    /// longer, and the count badge still reports the whole queue.
    #[test]
    fn the_stack_shows_max_visible_cards_and_counts_them_all() {
        let (w, _) = laid_out(false);
        assert_eq!(items().len(), 4);
        assert_eq!(w.rows.len(), NOTIFICATION_STACK_MAX_VISIBLE);
        assert_eq!(w.total, 4);
        assert_eq!(w.count.content(), "4");

        let mut floored = build_from(&view(false).max_visible(0));
        layout(&mut floored);
        assert_eq!(floored.rows.len(), 1, "maxVisible is floored at one");
    }

    /// Collapsed, the cards sit on the peek/inset geometry `notification-stack`
    /// authors — the two constants the toast stack borrowed back.
    #[test]
    fn the_collapsed_stack_peeks_and_insets_by_depth() {
        let (mut w, size) = laid_out(false);
        settle(&mut w, size, 0.0);
        let first = w.card_rect(0).unwrap();
        let second = w.card_rect(1).unwrap();
        let third = w.card_rect(2).unwrap();

        assert_eq!(first.y0 - NOTIFICATION_STACK_PADDING, toast_stack_peek(0));
        assert_eq!(second.y0 - NOTIFICATION_STACK_PADDING, toast_stack_peek(1));
        assert_eq!(third.y0 - NOTIFICATION_STACK_PADDING, toast_stack_peek(2));
        assert!(
            second.width() < first.width() && third.width() < second.width(),
            "a deeper card is inset on both sides: {first:?} {second:?} {third:?}"
        );
        assert_eq!(first.x0, NOTIFICATION_STACK_PADDING);
        // Symmetric, both sides — the `inset(0px Npx)` shorthand.
        assert!(
            (first.x0 + first.width() - (second.x0 + second.width() + 12.0)).abs() < 0.01,
            "the inset is taken off both edges"
        );
    }

    /// Expanded, the cards become a gapped column in the same order, with no
    /// peek and no inset left.
    #[test]
    fn the_expanded_stack_is_a_gapped_column_in_the_same_order() {
        let (mut w, size) = laid_out(false);
        let size = settle(&mut w, size, 0.0);
        let collapsed_ids: Vec<_> = w.rows.iter().map(|r| r.id.clone()).collect();

        retarget(&mut w, false, true);
        settle(&mut w, size, 20_000.0);

        let expanded_ids: Vec<_> = w.rows.iter().map(|r| r.id.clone()).collect();
        assert_eq!(collapsed_ids, expanded_ids, "expanding preserves order");

        let rects: Vec<_> = (0..w.rows.len()).map(|i| w.card_rect(i).unwrap()).collect();
        for pair in rects.windows(2) {
            assert!(
                (pair[1].y0 - pair[0].y1 - NOTIFICATION_STACK_GRID_GAP).abs() < 0.01,
                "cards are gapped, not overlapped: {:?} then {:?}",
                pair[0],
                pair[1]
            );
            assert_eq!(pair[0].x0, pair[1].x0, "no inset survives the spread");
            assert_eq!(pair[0].width(), pair[1].width());
        }
    }

    /// The box grows with the spread — the divergence the module docs record —
    /// and is strictly between the two heights mid-flight.
    #[test]
    fn the_box_grows_between_the_collapsed_and_expanded_heights() {
        let (mut w, size) = laid_out(false);
        let collapsed = settle(&mut w, size, 0.0);
        assert_eq!(collapsed.height, w.collapsed_height);
        assert!(w.expanded_height > w.collapsed_height);

        retarget(&mut w, false, true);
        let size = layout(&mut w);
        assert_eq!(
            size.height, collapsed.height,
            "the first frame has not moved"
        );
        let (_, needs_frame) = paint_at(&mut w, size, 20_000.0);
        assert!(needs_frame, "a spreading stack owes frames");
        let mut size = layout(&mut w);
        for step in 1..4 {
            paint_at(&mut w, size, 20_000.0 + step as f64 * 50.0);
            size = layout(&mut w);
        }
        assert!(
            size.height > w.collapsed_height && size.height < w.expanded_height,
            "mid-spread {size:?} between {} and {}",
            w.collapsed_height,
            w.expanded_height
        );

        let settled = settle(&mut w, size, 21_000.0);
        assert!((settled.height - w.expanded_height).abs() < 0.01);
    }

    /// The muted tray runs on its own shorter ramp than the cards — upstream's
    /// 0.26s against their 0.32s — and both are mid-flight together.
    #[test]
    fn the_tray_runs_on_a_shorter_ramp_than_the_cards() {
        assert!(
            NOTIFICATION_STACK_BACKGROUND_RAMP.settle() < NOTIFICATION_STACK_CARD_RAMP.settle(),
            "the tray's ramp is the shorter of the two"
        );
        let (mut w, size) = laid_out(false);
        let size = settle(&mut w, size, 0.0);
        retarget(&mut w, false, true);
        layout(&mut w);
        paint_at(&mut w, size, 20_000.0);
        layout(&mut w);
        paint_at(&mut w, size, 20_140.0);
        let tray = w.background.value();
        assert!(tray > 0.0 && tray < 1.0, "the tray is mid-flight: {tray}");
        assert!(
            w.rows[1].peek.value() > 0.0,
            "the cards have not landed yet"
        );
    }

    /// A deeper card's text is hidden while collapsed and shown once expanded —
    /// upstream's plain `invisible` class, a hard cut.
    #[test]
    fn a_deep_cards_text_appears_only_once_expanded() {
        let (mut w, size) = laid_out(false);
        let size = settle(&mut w, size, 0.0);
        assert!(w.content_visible(0));
        assert!(!w.content_visible(1) && !w.content_visible(2));
        let (collapsed_rec, _) = paint_at(&mut w, size, 10_000.0);

        retarget(&mut w, false, true);
        let staged = layout(&mut w);
        let size = settle(&mut w, staged, 20_000.0);
        assert!(w.content_visible(1) && w.content_visible(2));
        let (expanded_rec, _) = paint_at(&mut w, size, 40_000.0);
        assert!(
            expanded_rec.inks.len() > collapsed_rec.inks.len(),
            "expanding reveals more text: {} vs {}",
            expanded_rec.inks.len(),
            collapsed_rec.inks.len()
        );
    }

    /// A hovering pointer asks to expand; one that leaves asks to collapse. The
    /// widget never writes the flag itself.
    #[test]
    fn hover_asks_to_expand_and_leaving_asks_to_collapse() {
        let (mut w, size) = laid_out(false);
        let mut state = App::default();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(20.0, 20.0)),
            &mut state,
        );
        assert_eq!(state.expanded, Some(true));
        assert!(!w.expanded, "the widget never writes its own flag");

        let (mut open, size) = laid_out(true);
        let mut state = App::default();
        dispatch(
            &mut open,
            size,
            &pointer(PointerPhase::Move, Point::new(-5.0, -5.0)),
            &mut state,
        );
        assert_eq!(state.expanded, Some(false));
    }

    /// A pointer that leaves without a `Move` — the only signal is
    /// `PaintCtx::is_hovered` — collapses through the drainable latch, one
    /// event pass later.
    #[test]
    fn a_hover_lost_at_paint_collapses_on_the_next_event_pass() {
        let (mut open, size) = laid_out(true);
        let mut state = App::default();
        // The pointer is on the stack; `PaintCtx::for_test` reports no hover
        // link, which is exactly the "it left" signal.
        open.hovered = true;
        paint_at(&mut open, size, 0.0);
        assert_eq!(
            open.pending_expanded,
            Some(false),
            "the collapse is recorded, not fired from paint"
        );
        assert_eq!(state.expanded_calls, 0);

        dispatch(
            &mut open,
            size,
            &pointer(PointerPhase::Move, Point::new(-5.0, -5.0)),
            &mut state,
        );
        assert_eq!(state.expanded, Some(false));
        assert_eq!(open.pending_expanded, None, "the latch drains once");
    }

    /// Keyboard focus holds the stack open under a leaving pointer — upstream's
    /// `hasFocus` guard.
    #[test]
    fn focus_holds_the_stack_open_under_a_leaving_pointer() {
        let (mut w, size) = laid_out(true);
        let mut state = App::default();
        w.hovered = true;
        w.focused = true;
        paint_at(&mut w, size, 0.0);
        assert_eq!(
            w.pending_expanded, None,
            "a focused stack does not collapse when the pointer leaves"
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(-5.0, -5.0)),
            &mut state,
        );
        assert_eq!(state.expanded_calls, 0);
    }

    /// The press machine: the first press expands, the second follows
    /// `on_view_all` when there is one and collapses when there is not.
    #[test]
    fn the_second_press_follows_view_all_or_collapses() {
        let at = Point::new(20.0, 20.0);
        let (mut w, size) = laid_out(false);
        let mut state = App::default();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.expanded, Some(true));
        assert_eq!(state.view_all_calls, 0);

        // Expanded, with no `on_view_all`: the press collapses.
        let (mut open, size) = laid_out(true);
        let mut state = App::default();
        dispatch(
            &mut open,
            size,
            &pointer(PointerPhase::Down, at),
            &mut state,
        );
        dispatch(&mut open, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.expanded, Some(false));
        assert_eq!(state.view_all_calls, 0);

        // Expanded, with one: the press follows it and the stack stays open.
        let mut with_all = build_from(&view_with_all(true));
        let size = layout(&mut with_all);
        let mut state = App::default();
        dispatch(
            &mut with_all,
            size,
            &pointer(PointerPhase::Down, at),
            &mut state,
        );
        dispatch(
            &mut with_all,
            size,
            &pointer(PointerPhase::Up, at),
            &mut state,
        );
        assert_eq!(state.view_all_calls, 1);
        assert_eq!(
            state.expanded_calls, 0,
            "following the link never collapses"
        );
    }

    /// A release outside the box reports nothing, and a cancelled press is
    /// dropped.
    #[test]
    fn a_release_outside_the_box_reports_nothing() {
        let (mut w, size) = laid_out(false);
        let mut state = App::default();
        let at = Point::new(20.0, 20.0);
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(-20.0, -20.0)),
            &mut state,
        );
        assert_eq!(state.expanded_calls, 0);

        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Cancel, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.expanded_calls, 0, "a cancelled press never activates");
    }

    /// Escape collapses an expanded stack and is ignored by a collapsed one;
    /// keyboard activation drives the same machine a press does.
    #[test]
    fn escape_collapses_and_activation_drives_the_same_machine() {
        let (mut open, size) = laid_out(true);
        let mut state = App::default();
        dispatch(&mut open, size, &key_event(NamedKey::Escape), &mut state);
        assert_eq!(state.expanded, Some(false));

        let (mut closed, size) = laid_out(false);
        let mut state = App::default();
        dispatch(&mut closed, size, &key_event(NamedKey::Escape), &mut state);
        assert_eq!(state.expanded_calls, 0);
        dispatch(&mut closed, size, &key_event(NamedKey::Enter), &mut state);
        assert_eq!(state.expanded, Some(true));
        assert!(closed.focused, "activating focuses the stack");
    }

    /// The footer label rolls between the two states: both are on screen
    /// mid-roll, clipped to the footer band, and the exit is the shorter ramp.
    #[test]
    fn the_footer_label_rolls_between_the_two_states() {
        let (mut w, size) = laid_out(false);
        let size = settle(&mut w, size, 0.0);
        assert_eq!(w.roll.value(), 0.0);

        retarget(&mut w, false, true);
        layout(&mut w);
        paint_at(&mut w, size, 20_000.0);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, size, 20_120.0);
        let roll = w.roll.value();
        assert!(roll > 0.0 && roll < 1.0, "mid-roll: {roll}");
        assert!(
            !rec.clips.is_empty(),
            "the rolling labels are clipped to the footer band"
        );

        settle(&mut w, size, 21_000.0);
        assert_eq!(w.roll.value(), 1.0);
        assert!(NOTIFICATION_STACK_ROLL_EXIT.settle() < NOTIFICATION_STACK_ROLL_ENTER.settle());
    }

    /// The footer travels down with the spread rather than staying put — the
    /// visible consequence of the grow-instead-of-overflow divergence.
    #[test]
    fn the_footer_travels_with_the_spread() {
        let (mut w, size) = laid_out(false);
        let size = settle(&mut w, size, 0.0);
        let collapsed = w.footer_rect();
        retarget(&mut w, false, true);
        let staged = layout(&mut w);
        settle(&mut w, staged, 20_000.0);
        let expanded = w.footer_rect();
        assert!(
            expanded.y0 > collapsed.y0,
            "the footer moved down: {} → {}",
            collapsed.y0,
            expanded.y0
        );
        assert!(
            (expanded.y1 - w.expanded_height + NOTIFICATION_STACK_PADDING).abs() < 0.01,
            "and lands one padding above the bottom edge"
        );
        let _ = size;
    }

    /// An empty queue paints the washed empty tray with its drawn `BellOff`
    /// mark, handles no input, and publishes a status node rather than a button.
    #[test]
    fn an_empty_queue_paints_the_empty_state() {
        let mut w = build_from(&notification_stack::<App, _>(
            Vec::new(),
            false,
            |s: &mut App, next: bool| {
                s.expanded = Some(next);
                s.expanded_calls += 1;
            },
        ));
        let size = layout(&mut w);
        assert!(w.is_empty());
        let (rec, needs_frame) = paint_at(&mut w, size, 0.0);
        assert!(!needs_frame, "the empty state animates nothing");
        let wash = style::with_alpha(crate::BEUI_LIGHT.muted, NOTIFICATION_STACK_EMPTY_WASH);
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].3, wash);
        assert!(
            !rec.strokes.is_empty()
                && rec
                    .strokes
                    .iter()
                    .all(|c| *c == crate::BEUI_LIGHT.muted_foreground),
            "the bell-off mark is drawn in the dim ink"
        );

        let mut state = App::default();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, Point::new(10.0, 10.0)),
            &mut state,
        );
        assert_eq!(state.expanded_calls, 0, "an empty stack is inert");
    }

    /// Every painted colour is a resolved token: the tray, the card, the
    /// hairline at 60%, and the count badge on the catalog's `--warning`
    /// (upstream's raw `orange-600`).
    #[test]
    fn every_surface_paints_its_token() {
        let (mut w, size) = laid_out(false);
        let size = settle(&mut w, size, 0.0);
        let (rec, _) = paint_at(&mut w, size, 10_000.0);
        let p = crate::BEUI_LIGHT;
        assert_eq!(rec.rrects[0].3, p.muted, "the tray");
        assert!(rec.rrects.iter().any(|(_, _, _, c)| *c == p.background));
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == BeuiTokens::beui().warning),
            "the count badge takes `--warning`, not a raw Tailwind orange"
        );
        assert!(
            rec.strokes
                .iter()
                .any(|c| *c == style::with_alpha(p.border, NOTIFICATION_STACK_BORDER_ALPHA)),
            "the card hairline is washed to 60%"
        );
    }

    /// `reduce_motion` collapses every ramp: the box lands on its target height
    /// on the first frame and owes no frame.
    #[test]
    fn reduced_motion_lands_the_spread_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(false);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);

        retarget(&mut w, false, true);
        let size = layout(&mut w);
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(16.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(!ctx.needs_frame(), "a collapsed ramp owes no frame");
        assert_eq!(layout(&mut w).height, w.expanded_height);
        assert_eq!(w.rows[2].peek.value(), 0.0);
        assert_eq!(w.rows[2].inset.value(), 0.0);
    }

    /// The stack publishes one button, named by the count and by what a press
    /// would do next, carrying its expanded state.
    #[test]
    fn semantics_publish_one_named_expandable_button() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = App::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut App| view(false);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(600.0, 800.0), &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 1, "the whole stack is one button");
        assert_eq!(
            buttons[0].1.label(),
            Some("4 notifications. Expand notifications.")
        );
        assert_eq!(buttons[0].1.is_expanded(), Some(false));
    }

    // ---- Typeface: cards, the header and the empty line follow the theme ---

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(480.0, 600.0);

    /// Two cards — one with a description and a trailing stamp — collapsed or
    /// `expanded`, or none at all.
    fn probe_logic(
        count: usize,
        expanded: bool,
    ) -> impl FnMut(&mut ()) -> NotificationStackView<()> {
        move |_: &mut ()| {
            let items = [
                notification("a", "Sarah commented")
                    .description("Left three notes")
                    .trailing("2m"),
                notification("b", "Build passed"),
            ];
            notification_stack::<(), _>(items[..count].to_vec(), expanded, |_: &mut (), _| {})
                .on_view_all(|_: &mut ()| {})
        }
    }

    /// Each probed state, described.
    const STATES: [(usize, bool, &str); 3] = [
        (2, true, "the expanded stack's text"),
        (2, false, "the collapsed stack's text"),
        (0, false, "the empty line"),
    ];

    #[test]
    fn stack_text_paints_in_geist_under_the_beui_theme() {
        for (count, expanded, what) in STATES {
            assert_paints_only_in_geist(what, probe_logic(count, expanded), PROBE_WINDOW);
        }
        let expanded = Probe::new(probe_logic(2, true), PROBE_WINDOW, crate::theme()).frame();
        assert_eq!(
            expanded.len(),
            6,
            "two titles, a description, a stamp, the count and the header label"
        );
    }

    #[test]
    fn stack_text_follows_a_live_theme_family_swap() {
        for (count, expanded, what) in STATES {
            assert_follows_a_live_family_swap(what, probe_logic(count, expanded), PROBE_WINDOW);
        }
    }
}
