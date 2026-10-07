//! Ports beUI's `availability-scheduler` composed block — the weekly
//! availability editor: one row per weekday, each springing between available
//! and unavailable, its time ranges adding and removing on their own ramps.
//!
//! Source: `components/motion/availability-scheduler/` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01) — six
//! files: `index.tsx`, `day-row.tsx`, `time-select.tsx`, `copy-menu.tsx`,
//! `icon-button.tsx` and `types.ts`. Registry slug `availability-scheduler`
//! (blocks): *"Weekly availability editor where each day springs between
//! available and unavailable, time ranges add and remove with blur-slide
//! motion, times pick from a scrollable dropdown, and a copy menu clones hours
//! to other days."*
//!
//! # Premise correction: this is not a drag-select grid
//!
//! It is worth stating outright, because the slug's name invites the other
//! reading: **upstream has no week grid, no time-slot cells and no drag
//! selection**. There is no pointer-drag gesture anywhere in the six files, and
//! nothing that toggles a cell. What it has is a list of seven day rows, each
//! carrying a [`switch`](crate::components::switch), zero or more
//! `{ start, end }` ranges edited through two dropdowns, an add-range button and
//! a copy-to-other-days menu. This port is of that component. The selection
//! model it exposes is [`WeekAvailability`] — days and ranges — because that is
//! the model the source has.
//!
//! | class / prop | here |
//! |---|---|
//! | root `w-full max-w-xl divide-y divide-border` | [`SCHEDULER_MAX_WIDTH`], one hairline between rows |
//! | row `py-4 sm:gap-4`, label column `sm:w-36` | [`ROW_PADDING_Y`] / [`ROW_GAP`] / [`LABEL_COLUMN`] |
//! | switch + label `gap-2.5`, `text-sm font-medium` | [`SWITCH_LABEL_GAP`] / [`style::TEXT_SM`] |
//! | ranges `flex-col gap-2`, each `flex items-center gap-2` | [`RANGE_GAP`] / [`RANGE_INNER_GAP`] |
//! | time field `sm:max-w-[132px]`, `tabular-nums` | [`TIME_FIELD_MAX_WIDTH`], [`style::HEIGHT_MD`], [`style::RADIUS_CONTROL`] |
//! | `–` separator `text-muted-foreground` | [`DASH_WIDTH`] in `on_surface_variant` |
//! | icon button `h-8 w-8 rounded-lg`, `whileTap: scale 0.86` | [`ICON_BUTTON_BOX`] / [`style::RADIUS_LG`] / [`ICON_BUTTON_PRESS_SCALE`] |
//! | icon button `hover:bg-muted`, `disabled:opacity-40` | `surface_container_highest` / [`ICON_BUTTON_DISABLED_OPACITY`] |
//! | range enter `y: -6`, exit `y: -4`, `SPRING_LAYOUT` | [`RANGE_ENTER_RISE`] / [`RANGE_EXIT_RISE`] / [`SPRING_LAYOUT`] |
//! | `Unavailable` `py-1 text-sm text-muted-foreground` | the same presence, [`style::TEXT_SM`] |
//!
//! # The two overlays are the app's, exactly as the catalog's own are
//!
//! Upstream's time fields are `<Select>`s and its copy menu is a morph popover.
//! Both are **overlay-hosted** in this catalog — `select`'s panel and
//! `combobox`'s are separate views an app mounts in its own overlay layer, with
//! the trigger reporting open/closed — so a scheduler that hosted them inline
//! would be inventing a second hosting mechanism.
//!
//! It therefore does what `index.tsx` itself does: **it holds the one open panel
//! for the whole week**, reports which one through
//! [`AvailabilitySchedulerView::on_panel_open_change`], and leaves the app to
//! mount [`select`](crate::components::select) against it. Upstream's own reason
//! for centralising that is preserved in [`AvailabilitySchedulerView::open_panel`]'s
//! docs. [`start_options`]/[`end_options`] are what the app feeds that panel, and
//! [`clamp_range`] is what it applies to the result; the copy picker is the same
//! shape through [`AvailabilitySchedulerView::on_copy_request`] and [`copy_day`].
//!
//! # Degradations
//!
//! - **No tooltips.** Every icon button upstream is wrapped in a `Tooltip`;
//!   tooltips are anchored overlays, and the same hosting argument applies.
//!   The buttons carry accessible names instead.
//! - **No blur on a range's enter/exit.** `filter: blur(4px)` has no counterpart
//!   in this scene's paint vocabulary; the slide and fade are ported.
//! - **No elevation reordering.** `zIndex` on the row holding the open panel is
//!   meaningless without an inline panel to raise above its neighbours.

use std::rc::Rc;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any,
    build_child, erase_callback_arg, rebuild_children, route_event, teardown_child, visit_children,
};
use frust::{ChildKey, FrameTime, Theme};

use crate::components::switch::switch;
use crate::motion::{Presence, Ramp};
use crate::press::{Lane, press_scale, presses};
use crate::style;
use crate::text::{Label, ThemeTextType};
use crate::tokens::motion::{SPRING_LAYOUT, SPRING_PRESS};
use crate::tokens::sans_family;

// ---- Ported metrics --------------------------------------------------------

/// The scheduler's width ceiling, in logical px (`max-w-xl`).
pub const SCHEDULER_MAX_WIDTH: f64 = 576.0;
/// A day row's vertical padding, in logical px (`py-4`).
pub const ROW_PADDING_Y: f64 = 16.0;
/// Gap between a row's three columns, in logical px (`sm:gap-4`).
pub const ROW_GAP: f64 = 16.0;
/// The label column's width, in logical px (`sm:w-36`).
pub const LABEL_COLUMN: f64 = 144.0;
/// Gap between the switch and the day's name, in logical px (`gap-2.5`).
pub const SWITCH_LABEL_GAP: f64 = 10.0;
/// Gap between two time ranges, in logical px (`gap-2`).
pub const RANGE_GAP: f64 = 8.0;
/// Gap inside one range row, in logical px (`gap-2`).
pub const RANGE_INNER_GAP: f64 = 8.0;
/// A time field's width ceiling, in logical px (`sm:max-w-[132px]`).
pub const TIME_FIELD_MAX_WIDTH: f64 = 132.0;
/// The `–` separator's column width, in logical px.
pub const DASH_WIDTH: f64 = 10.0;
/// An icon button's box, in logical px (`h-8 w-8`).
pub const ICON_BUTTON_BOX: f64 = 32.0;
/// An icon button's glyph edge, in logical px (`h-4 w-4`).
pub const ICON_BUTTON_ICON: f64 = style::ICON_SIZE;
/// Gap between two icon buttons, in logical px (`gap-1`).
pub const ICON_BUTTON_GAP: f64 = 4.0;
/// The scale an icon button shrinks to while pressed
/// (`whileTap={{ scale: 0.86 }}`).
pub const ICON_BUTTON_PRESS_SCALE: f64 = 0.86;
/// Opacity of a disabled icon button (`disabled:opacity-40`).
pub const ICON_BUTTON_DISABLED_OPACITY: f32 = 0.4;
/// The chevron's edge inside a time field, in logical px.
pub const TIME_FIELD_CHEVRON: f64 = style::ICON_SIZE;

/// How far a range rises into place, in logical px (`y: -6 → 0`).
pub const RANGE_ENTER_RISE: f64 = 6.0;
/// How far a range rises as it leaves, in logical px (`y: -4`).
pub const RANGE_EXIT_RISE: f64 = 4.0;

/// Minutes in a day — the ceiling every time value is clamped under.
pub const MINUTES_PER_DAY: u32 = 24 * 60;
/// The step [`build_options`] uses when a caller names none (`step = 30`).
pub const DEFAULT_STEP_MINUTES: u32 = 30;
/// A new range's length, in minutes (`start + 60`).
pub const NEW_RANGE_MINUTES: u32 = 60;
/// The default day's opening time (`"09:00"`).
pub const DEFAULT_START: &str = "09:00";
/// The default day's closing time (`"17:00"`).
pub const DEFAULT_END: &str = "17:00";

/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dim ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback muted fill — the light table's `--muted`.
const FALLBACK_MUTED_FILL: Color = crate::BEUI_LIGHT.muted;

// ---- The week model --------------------------------------------------------

/// One of the seven weekdays — upstream's `DayKey`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DayKey {
    /// `"mon"`.
    Mon,
    /// `"tue"`.
    Tue,
    /// `"wed"`.
    Wed,
    /// `"thu"`.
    Thu,
    /// `"fri"`.
    Fri,
    /// `"sat"`.
    Sat,
    /// `"sun"`.
    Sun,
}

impl DayKey {
    /// The week, Monday first — upstream's `WEEKDAYS`.
    pub const WEEK: [DayKey; 7] = [
        DayKey::Mon,
        DayKey::Tue,
        DayKey::Wed,
        DayKey::Thu,
        DayKey::Fri,
        DayKey::Sat,
        DayKey::Sun,
    ];

    /// The day's short key, as upstream writes it (`"mon"`).
    pub const fn key(self) -> &'static str {
        match self {
            DayKey::Mon => "mon",
            DayKey::Tue => "tue",
            DayKey::Wed => "wed",
            DayKey::Thu => "thu",
            DayKey::Fri => "fri",
            DayKey::Sat => "sat",
            DayKey::Sun => "sun",
        }
    }

    /// The day's displayed name (`"Monday"`).
    pub const fn label(self) -> &'static str {
        match self {
            DayKey::Mon => "Monday",
            DayKey::Tue => "Tuesday",
            DayKey::Wed => "Wednesday",
            DayKey::Thu => "Thursday",
            DayKey::Fri => "Friday",
            DayKey::Sat => "Saturday",
            DayKey::Sun => "Sunday",
        }
    }

    /// The day's index into a [`WeekAvailability`].
    pub const fn index(self) -> usize {
        match self {
            DayKey::Mon => 0,
            DayKey::Tue => 1,
            DayKey::Wed => 2,
            DayKey::Thu => 3,
            DayKey::Fri => 4,
            DayKey::Sat => 5,
            DayKey::Sun => 6,
        }
    }
}

/// One `{ id, start, end }` window — upstream's `TimeRange`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeRange {
    /// The range's identity within its day, which is what a
    /// [`panel_key`] and a row's presence are matched by.
    pub id: String,
    /// The opening time, `"HH:MM"`.
    pub start: String,
    /// The closing time, `"HH:MM"`.
    pub end: String,
}

impl TimeRange {
    /// A range with `id` running `start` → `end`.
    pub fn new(id: impl Into<String>, start: impl Into<String>, end: impl Into<String>) -> Self {
        TimeRange {
            id: id.into(),
            start: start.into(),
            end: end.into(),
        }
    }
}

/// One day's availability — upstream's `DayAvailability`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayAvailability {
    /// Whether the day is available at all (the row's switch).
    pub enabled: bool,
    /// The day's windows, in order.
    pub ranges: Vec<TimeRange>,
}

/// The whole week — upstream's `WeekAvailability`, as a fixed seven-slot array
/// rather than a keyed record.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WeekAvailability {
    days: [DayAvailability; 7],
}

impl WeekAvailability {
    /// Read one day.
    pub fn day(&self, day: DayKey) -> &DayAvailability {
        &self.days[day.index()]
    }

    /// Replace one day.
    pub fn set_day(&mut self, day: DayKey, value: DayAvailability) {
        self.days[day.index()] = value;
    }

    /// A copy with `day` replaced — the shape a controlled callback reports.
    pub fn with_day(&self, day: DayKey, value: DayAvailability) -> Self {
        let mut next = self.clone();
        next.set_day(day, value);
        next
    }
}

/// The starting week — upstream's `defaultWeek`: Monday to Friday 09:00–17:00,
/// the weekend off but carrying the same (unused) window, and fixed ids so a
/// first render agrees with itself.
pub fn default_week() -> WeekAvailability {
    let mut week = WeekAvailability::default();
    for day in DayKey::WEEK {
        week.set_day(
            day,
            DayAvailability {
                enabled: !matches!(day, DayKey::Sat | DayKey::Sun),
                ranges: vec![TimeRange::new(
                    format!("{}-0", day.key()),
                    DEFAULT_START,
                    DEFAULT_END,
                )],
            },
        );
    }
    week
}

/// Which end of a range a panel edits — upstream's `"start" | "end"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeEdge {
    /// The opening time.
    Start,
    /// The closing time.
    End,
}

impl TimeEdge {
    /// The word upstream writes into a [`panel_key`].
    pub const fn key(self) -> &'static str {
        match self {
            TimeEdge::Start => "start",
            TimeEdge::End => "end",
        }
    }
}

/// Name one time field week-wide — upstream's `panelKey`.
///
/// The scheduler holds a single open panel for the whole week, and range ids
/// are only unique *within* a day, so the day is part of the name.
pub fn panel_key(day: DayKey, range_id: &str, edge: TimeEdge) -> String {
    format!("{}:{}:{}", day.key(), range_id, edge.key())
}

/// Parse `"HH:MM"` into minutes past midnight — upstream's `toMinutes`.
///
/// A malformed value reads as midnight rather than panicking, which is the
/// closest thing to `NaN`-free behaviour upstream's own `Number()` split has.
pub fn to_minutes(value: &str) -> u32 {
    let Some((hours, minutes)) = value.split_once(':') else {
        return 0;
    };
    let hours: u32 = hours.trim().parse().unwrap_or(0);
    let minutes: u32 = minutes.trim().parse().unwrap_or(0);
    hours * 60 + minutes
}

/// Format minutes past midnight as `"HH:MM"`, clamped into the day — upstream's
/// `toValue`.
pub fn to_value(minutes: u32) -> String {
    let clamped = minutes.min(MINUTES_PER_DAY - 1);
    format!("{:02}:{:02}", clamped / 60, clamped % 60)
}

/// Format `"HH:MM"` as a twelve-hour label — upstream's `label12`
/// (`"09:00"` → `"9:00 AM"`).
pub fn label_12(value: &str) -> String {
    let minutes = to_minutes(value);
    let hours = minutes / 60;
    let meridiem = if hours < 12 { "AM" } else { "PM" };
    let hour_12 = match hours % 12 {
        0 => 12,
        other => other,
    };
    format!("{hour_12}:{:02} {meridiem}", minutes % 60)
}

/// Every selectable time at `step` minutes — upstream's `buildOptions`.
///
/// A zero (or absurd) step would produce an unbounded list, so it is floored at
/// one minute; upstream has no such guard because its own caller never passes
/// one.
pub fn build_options(step: u32) -> Vec<String> {
    let step = step.max(1);
    (0..MINUTES_PER_DAY)
        .step_by(step as usize)
        .map(to_value)
        .collect()
}

/// Restore the `end > start` invariant on a same-day range — upstream's
/// `clampRange`.
///
/// A valid pair is left **exactly** alone, so an off-grid persisted end (17:00
/// against a 12-hour step) is not rewritten. When the invariant fails, the
/// just-`changed` endpoint stays put and the opposite one moves onto a
/// neighbouring generated option; a midnight end or a last-slot start, which
/// cannot bound a positive range at all, fall back to the first or last pair.
pub fn clamp_range(
    start: &str,
    end: &str,
    options: &[String],
    changed: TimeEdge,
) -> (String, String) {
    let slots: Vec<u32> = options.iter().map(|o| to_minutes(o)).collect();
    if slots.is_empty() {
        return (start.to_string(), end.to_string());
    }
    if to_minutes(end) > to_minutes(start) {
        return (start.to_string(), end.to_string());
    }

    let keep_or_snap = |value: &str| {
        let minutes = to_minutes(value);
        if slots.contains(&minutes) {
            minutes
        } else {
            snap_to_option(minutes, &slots)
        }
    };

    match changed {
        TimeEdge::End => {
            let e = keep_or_snap(end);
            match slots.iter().rev().find(|slot| **slot < e) {
                Some(earlier) => (to_value(*earlier), to_value(e)),
                None if slots.len() > 1 => {
                    // Midnight cannot end a positive same-day range.
                    (to_value(slots[0]), to_value(slots[1]))
                }
                None => (to_value(keep_or_snap(start)), to_value(e)),
            }
        }
        TimeEdge::Start => {
            let s = keep_or_snap(start);
            match slots.iter().find(|slot| **slot > s) {
                Some(later) => (to_value(s), to_value(*later)),
                None if slots.len() > 1 => {
                    // The last slot cannot start a positive range.
                    (
                        to_value(slots[slots.len() - 2]),
                        to_value(slots[slots.len() - 1]),
                    )
                }
                None => (to_value(s), to_value(keep_or_snap(end))),
            }
        }
    }
}

/// The nearest option to `minutes` — upstream's `snapToOption`.
fn snap_to_option(minutes: u32, slots: &[u32]) -> u32 {
    let mut best = slots[0];
    for slot in slots {
        if slot.abs_diff(minutes) < best.abs_diff(minutes) {
            best = *slot;
        }
    }
    best
}

/// Add `current` back to a filtered option list when the filter dropped it —
/// upstream's `withCurrentOption`, so a persisted off-grid value stays
/// selectable.
fn with_current_option(mut filtered: Vec<String>, current: Option<&str>) -> Vec<String> {
    let Some(current) = current else {
        return filtered;
    };
    if filtered.iter().any(|o| o == current) {
        return filtered;
    }
    filtered.push(current.to_string());
    filtered.sort_by_key(|o| to_minutes(o));
    filtered
}

/// The options a range's **start** field may show — upstream's `startOptions`:
/// everything before `end`, with a midnight-end recovery and the current value
/// kept selectable.
pub fn start_options(options: &[String], end: &str, current: Option<&str>) -> Vec<String> {
    let limit = to_minutes(end);
    let filtered: Vec<String> = options
        .iter()
        .filter(|o| to_minutes(o) < limit)
        .cloned()
        .collect();
    // An invalid midnight end has no earlier option; exposing midnight lets
    // choosing it move the end forward through `clamp_range` instead of
    // trapping the row.
    let recovery = if filtered.is_empty() {
        options.iter().take(1).cloned().collect()
    } else {
        filtered
    };
    with_current_option(recovery, current)
}

/// The options a range's **end** field may show — upstream's `endOptions`:
/// everything after `start`, with a last-slot recovery and the current value
/// kept selectable.
pub fn end_options(options: &[String], start: &str, current: Option<&str>) -> Vec<String> {
    let floor = to_minutes(start);
    let filtered: Vec<String> = options
        .iter()
        .filter(|o| to_minutes(o) > floor)
        .cloned()
        .collect();
    let recovery = if filtered.is_empty() {
        options.iter().rev().take(1).cloned().collect()
    } else {
        filtered
    };
    with_current_option(recovery, current)
}

/// Turn a day on or off — upstream's `setEnabled`: switching an empty day on
/// gives it the default 09:00–17:00 window rather than an availability with no
/// hours in it.
pub fn set_enabled(
    day: &DayAvailability,
    enabled: bool,
    next_id: impl FnOnce() -> String,
) -> DayAvailability {
    if enabled && day.ranges.is_empty() {
        return DayAvailability {
            enabled,
            ranges: vec![TimeRange::new(next_id(), DEFAULT_START, DEFAULT_END)],
        };
    }
    DayAvailability {
        enabled,
        ranges: day.ranges.clone(),
    }
}

/// Append a window — upstream's `addRange`: one hour long, starting an hour
/// after the last window ends and never later than an hour before midnight.
/// Adding to an off day switches it on.
pub fn add_range(day: &DayAvailability, next_id: impl FnOnce() -> String) -> DayAvailability {
    let start = match day.ranges.last() {
        Some(last) => {
            (to_minutes(&last.end) + NEW_RANGE_MINUTES).min(MINUTES_PER_DAY - NEW_RANGE_MINUTES)
        }
        // `540` — nine o'clock, upstream's own literal.
        None => to_minutes(DEFAULT_START),
    };
    let mut ranges = day.ranges.clone();
    ranges.push(TimeRange::new(
        next_id(),
        to_value(start),
        to_value(start + NEW_RANGE_MINUTES),
    ));
    DayAvailability {
        enabled: true,
        ranges,
    }
}

/// Drop the window with `id` — upstream's `removeRange`: removing the last one
/// marks the day unavailable.
pub fn remove_range(day: &DayAvailability, id: &str) -> DayAvailability {
    let ranges: Vec<TimeRange> = day
        .ranges
        .iter()
        .filter(|range| range.id != id)
        .cloned()
        .collect();
    DayAvailability {
        enabled: !ranges.is_empty(),
        ranges,
    }
}

/// Clone `from`'s hours onto `targets` — upstream's `copyDay`, with each copied
/// window given a fresh id so the two days' ranges stay distinguishable.
pub fn copy_day(
    week: &WeekAvailability,
    from: DayKey,
    targets: &[DayKey],
    mut next_id: impl FnMut(DayKey) -> String,
) -> WeekAvailability {
    let source = week.day(from).clone();
    let mut next = week.clone();
    for target in targets {
        next.set_day(
            *target,
            DayAvailability {
                enabled: source.enabled,
                ranges: source
                    .ranges
                    .iter()
                    .map(|range| TimeRange::new(next_id(*target), &range.start, &range.end))
                    .collect(),
            },
        );
    }
    next
}

// ---- View ------------------------------------------------------------------

/// A view-held week callback.
type OnWeek<State> = Rc<dyn Fn(&mut State, WeekAvailability)>;
/// A view-held open-panel callback (`None` closes whatever is open).
type OnPanel<State> = Rc<dyn Fn(&mut State, Option<String>)>;
/// A view-held copy-request callback.
type OnCopy<State> = Rc<dyn Fn(&mut State, DayKey)>;

/// A declarative beUI availability scheduler. See the [module docs](self).
pub struct AvailabilitySchedulerView<State: 'static> {
    value: WeekAvailability,
    step: u32,
    open_panel: Option<String>,
    on_change: OnWeek<State>,
    on_panel_open_change: Option<OnPanel<State>>,
    on_copy_request: Option<OnCopy<State>>,
}

/// Create a scheduler editing `value`, reporting every edit through
/// `on_change` — a **controlled** component (see the [module docs](self)).
pub fn availability_scheduler<State: 'static, F: Fn(&mut State, WeekAvailability) + 'static>(
    value: WeekAvailability,
    on_change: F,
) -> AvailabilitySchedulerView<State> {
    AvailabilitySchedulerView {
        value,
        step: DEFAULT_STEP_MINUTES,
        open_panel: None,
        on_change: Rc::new(on_change),
        on_panel_open_change: None,
        on_copy_request: None,
    }
}

impl<State: 'static> AvailabilitySchedulerView<State> {
    /// Set the minutes between selectable times (`step`, default
    /// [`DEFAULT_STEP_MINUTES`]).
    pub fn step(mut self, step: u32) -> Self {
        self.step = step.max(1);
        self
    }

    /// Name the one time panel the app currently has open, as a [`panel_key`].
    ///
    /// Exactly one panel is open at a time and the **scheduler** is what knows
    /// which — upstream's own reasoning, preserved: a panel is positioned
    /// inside its own field, so two open at once paint over each other's
    /// options, and nothing else can close the first (a select dismisses on an
    /// outside pointer-down, which keyboard and assistive-technology activation
    /// never fires).
    ///
    /// A key naming a field the week no longer puts on screen — its day
    /// switched off, its range removed — is reported closed on the next
    /// rebuild, so the panel cannot spring back when the same range returns.
    pub fn open_panel(mut self, key: Option<String>) -> Self {
        self.open_panel = key;
        self
    }

    /// Report which time panel should be open now (`onPanelOpenChange`).
    pub fn on_panel_open_change<F: Fn(&mut State, Option<String>) + 'static>(
        mut self,
        on_change: F,
    ) -> Self {
        self.on_panel_open_change = Some(Rc::new(on_change));
        self
    }

    /// Report a press on a day's copy affordance, for the app to host the day
    /// picker against (`CopyMenu`). Without it the affordance is not painted.
    pub fn on_copy_request<F: Fn(&mut State, DayKey) + 'static>(mut self, on_copy: F) -> Self {
        self.on_copy_request = Some(Rc::new(on_copy));
        self
    }

    /// The panel keys the week currently puts on screen — upstream's
    /// `livePanels`.
    pub fn live_panels(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for day in DayKey::WEEK {
            let state = self.value.day(day);
            if !state.enabled {
                continue;
            }
            for range in &state.ranges {
                keys.push(panel_key(day, &range.id, TimeEdge::Start));
                keys.push(panel_key(day, &range.id, TimeEdge::End));
            }
        }
        keys
    }

    /// One day's switch, reporting the whole week back.
    // erasure: keep feeds build_child/rebuild_child/teardown_child, which take &AnyView
    fn switch_view(&self, day: DayKey) -> AnyView<State> {
        let week = self.value.clone();
        let on_change = self.on_change.clone();
        let state = self.value.day(day).clone();
        any(
            switch::<State, _>(state.enabled, move |app: &mut State, enabled| {
                let next = set_enabled(&state, enabled, || format!("{}-n0", day.key()));
                on_change(app, week.with_day(day, next));
            })
            .label(format!("Toggle {} availability", day.label())),
        )
    }
}

// ---- Widget ----------------------------------------------------------------

/// One retained range row inside a day.
struct RangeRow {
    id: String,
    start: Label,
    end: Label,
    /// Kept-mounted enter/exit.
    presence: Presence,
    /// What [`Presence::advance`] last reported.
    shown: f64,
    /// Whether the week still carries this range.
    leaving: bool,
    /// The row's box in widget-local space.
    rect: Rect,
}

/// One retained day row.
struct DayRow {
    day: DayKey,
    enabled: bool,
    label: Label,
    unavailable: Label,
    ranges: Vec<RangeRow>,
    /// The row's box in widget-local space.
    rect: Rect,
    /// The `Unavailable` label's presence.
    absent: Presence,
    absent_shown: f64,
}

/// Which affordance a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// A time field: `(day index, range index, edge)`.
    Field(usize, usize, TimeEdge),
    /// A range's remove button.
    Remove(usize, usize),
    /// A day's add-range button.
    Add(usize),
    /// A day's copy button.
    Copy(usize),
}

/// The retained widget for an [`AvailabilitySchedulerView`].
pub struct AvailabilitySchedulerWidget {
    /// One switch pod per weekday, in [`DayKey::WEEK`] order.
    switches: Vec<ChildPod>,
    rows: Vec<DayRow>,
    open_panel: Option<String>,
    has_copy: bool,
    /// The affordance a `Down` armed, and its press shrink.
    armed: Option<Target>,
    press: Lane,
    /// The latched hovered affordance, self-corrected at paint time.
    hovered: Option<Target>,
    width: f64,
    height: f64,
    on_field_press: ErasedArgCallback<Option<String>>,
    on_week: ErasedArgCallback<WeekAvailability>,
    on_copy: Option<ErasedArgCallback<DayKey>>,
    /// The week as of the last rebuild — what a press computes its edit from.
    week: WeekAvailability,
    /// A monotone counter behind every generated range id, so two ranges added
    /// in one session never collide.
    next_id: u64,
}

impl RangeRow {
    fn new(range: &TimeRange) -> Self {
        let mut presence = Presence::symmetric(Ramp::spring(SPRING_LAYOUT));
        presence.set_open(true);
        RangeRow {
            id: range.id.clone(),
            start: Label::new(label_12(&range.start)),
            end: Label::new(label_12(&range.end)),
            presence,
            shown: 0.0,
            leaving: false,
            rect: Rect::ZERO,
        }
    }
}

impl<State: 'static> View<State> for AvailabilitySchedulerView<State> {
    type Element = AvailabilitySchedulerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AvailabilitySchedulerWidget {
        let settled =
            FrameTime::from_nanos(Ramp::spring(SPRING_LAYOUT).settle().as_nanos() as u64 + 1);
        let rows = DayKey::WEEK
            .into_iter()
            .map(|day| {
                let state = self.value.day(day);
                let mut absent = Presence::symmetric(Ramp::spring(SPRING_LAYOUT));
                absent.set_open(!state.enabled);
                absent.advance(FrameTime::ZERO);
                let absent_shown = absent.advance(settled);
                let mut ranges: Vec<RangeRow> = state.ranges.iter().map(RangeRow::new).collect();
                // `<AnimatePresence initial={false}>`: a week that mounts with
                // hours in it does not cascade them in — and a switched-off
                // day's hours are simply not on screen.
                for row in &mut ranges {
                    row.presence.set_open(state.enabled);
                    row.presence.advance(FrameTime::ZERO);
                    row.shown = row.presence.advance(settled);
                }
                DayRow {
                    day,
                    enabled: state.enabled,
                    label: Label::new(day.label()),
                    unavailable: Label::new("Unavailable"),
                    ranges,
                    rect: Rect::ZERO,
                    absent,
                    absent_shown,
                }
            })
            .collect();

        AvailabilitySchedulerWidget {
            switches: DayKey::WEEK
                .into_iter()
                .map(|day| build_child(&self.switch_view(day), ctx))
                .collect(),
            rows,
            open_panel: self.open_panel.clone(),
            has_copy: self.on_copy_request.is_some(),
            armed: None,
            press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            hovered: None,
            width: 0.0,
            height: 0.0,
            on_field_press: match &self.on_panel_open_change {
                Some(callback) => erase_callback_arg(callback),
                None => erase_callback_arg(&noop_panel::<State>()),
            },
            on_week: erase_callback_arg(&self.on_change),
            on_copy: self.on_copy_request.as_ref().map(erase_callback_arg),
            week: self.value.clone(),
            next_id: 0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AvailabilitySchedulerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_field_press = match &self.on_panel_open_change {
            Some(callback) => erase_callback_arg(callback),
            None => erase_callback_arg(&noop_panel::<State>()),
        };
        element.on_week = erase_callback_arg(&self.on_change);
        element.on_copy = self.on_copy_request.as_ref().map(erase_callback_arg);
        element.has_copy = self.on_copy_request.is_some();

        let prev_switches: Vec<AnyView<State>> = DayKey::WEEK
            .into_iter()
            .map(|d| prev.switch_view(d))
            .collect();
        let next_switches: Vec<AnyView<State>> = DayKey::WEEK
            .into_iter()
            .map(|d| self.switch_view(d))
            .collect();
        let mut flags = rebuild_children(
            &prev_switches,
            &next_switches,
            &mut element.switches,
            ctx,
            |view: &AnyView<State>| view,
            |_| None::<ChildKey>,
        );

        if prev.value != self.value {
            element.reconcile(&self.value);
            element.week = self.value.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.drop_exited() {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // A key naming a field the week no longer shows is dropped, so the
        // panel cannot spring back when the same range returns.
        let live = self.live_panels();
        let open = self
            .open_panel
            .clone()
            .filter(|key| live.iter().any(|k| k == key));
        if element.open_panel != open {
            element.open_panel = open;
            flags |= ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut AvailabilitySchedulerWidget, ctx: &mut BuildCtx<'_>) {
        for (day, pod) in DayKey::WEEK.into_iter().zip(element.switches.iter_mut()) {
            teardown_child(&self.switch_view(day), pod, ctx);
        }
    }
}

/// The no-op panel reporter a scheduler with no `on_panel_open_change` installs,
/// so the press arm never has to branch on whether one was supplied.
fn noop_panel<State: 'static>() -> OnPanel<State> {
    Rc::new(|_state: &mut State, _key: Option<String>| {})
}

impl AvailabilitySchedulerWidget {
    /// Match `week` onto the retained rows, by day and then by range id.
    fn reconcile(&mut self, week: &WeekAvailability) {
        for row in &mut self.rows {
            let state = week.day(row.day);
            if row.enabled != state.enabled {
                row.enabled = state.enabled;
                row.absent.set_open(!state.enabled);
            }

            for range_row in &mut row.ranges {
                if !state.ranges.iter().any(|r| r.id == range_row.id) && !range_row.leaving {
                    range_row.leaving = true;
                    range_row.presence.set_open(false);
                }
            }
            for range in &state.ranges {
                match row.ranges.iter_mut().find(|r| r.id == range.id) {
                    Some(existing) => {
                        if existing.leaving {
                            existing.leaving = false;
                            existing.presence.set_open(true);
                        }
                        existing.start.set_content(label_12(&range.start));
                        existing.end.set_content(label_12(&range.end));
                    }
                    None => row.ranges.push(RangeRow::new(range)),
                }
            }

            // A switched-off day shows the word `Unavailable`, not its hours —
            // so its ranges close even though the model still carries them.
            for range_row in &mut row.ranges {
                range_row
                    .presence
                    .set_open(state.enabled && !range_row.leaving);
            }

            // Re-order to the week's order, with a leaving row pinned to the
            // slot it is leaving from.
            let mut pinned: Vec<(usize, RangeRow)> = Vec::new();
            let mut live: Vec<RangeRow> = Vec::new();
            for (index, range_row) in row.ranges.drain(..).enumerate() {
                if range_row.leaving {
                    pinned.push((index, range_row));
                } else {
                    live.push(range_row);
                }
            }
            let mut ordered: Vec<RangeRow> = Vec::with_capacity(live.len());
            for range in &state.ranges {
                if let Some(at) = live.iter().position(|r| r.id == range.id) {
                    ordered.push(live.remove(at));
                }
            }
            ordered.extend(live);
            for (index, range_row) in pinned {
                let at = index.min(ordered.len());
                ordered.insert(at, range_row);
            }
            row.ranges = ordered;
        }
    }

    /// Drop every range row whose exit has finished. Reports whether any went.
    fn drop_exited(&mut self) -> bool {
        let mut dropped = false;
        for row in &mut self.rows {
            let before = row.ranges.len();
            row.ranges.retain(|r| !r.leaving || r.presence.is_visible());
            dropped |= row.ranges.len() != before;
        }
        dropped
    }

    /// A fresh range id for `day`, unique for the life of this widget.
    fn mint_id(&mut self, day: DayKey) -> String {
        self.next_id += 1;
        format!("{}-n{}", day.key(), self.next_id)
    }

    /// The time field's width for the resolved column.
    fn field_width(&self) -> f64 {
        let column = self.range_column_width();
        let actions = ICON_BUTTON_BOX + RANGE_INNER_GAP;
        let usable = (column - actions - DASH_WIDTH - RANGE_INNER_GAP * 2.0).max(2.0);
        (usable / 2.0).min(TIME_FIELD_MAX_WIDTH)
    }

    /// The width the range column gets, between the label column and the row's
    /// trailing actions.
    fn range_column_width(&self) -> f64 {
        let trailing = self.trailing_width();
        (self.width - LABEL_COLUMN - ROW_GAP - trailing - ROW_GAP).max(2.0)
    }

    /// The row's trailing action column: the add button, plus the copy one when
    /// the app supplied a handler for it.
    fn trailing_width(&self) -> f64 {
        if self.has_copy {
            ICON_BUTTON_BOX * 2.0 + ICON_BUTTON_GAP
        } else {
            ICON_BUTTON_BOX
        }
    }

    /// A range row's boxes: `(start field, end field, remove button)`.
    fn range_boxes(&self, row: &DayRow, range: &RangeRow) -> (Rect, Rect, Rect) {
        let field = self.field_width();
        let x = LABEL_COLUMN + ROW_GAP;
        let y = range.rect.y0;
        let height = style::HEIGHT_MD;
        let start = Rect::from_origin_size(Point::new(x, y), Size::new(field, height));
        let end = Rect::from_origin_size(
            Point::new(
                start.max_x() + RANGE_INNER_GAP + DASH_WIDTH + RANGE_INNER_GAP,
                y,
            ),
            Size::new(field, height),
        );
        let remove = Rect::from_origin_size(
            Point::new(
                end.max_x() + RANGE_INNER_GAP,
                y + (height - ICON_BUTTON_BOX) / 2.0,
            ),
            Size::new(ICON_BUTTON_BOX, ICON_BUTTON_BOX),
        );
        let _ = row;
        (start, end, remove)
    }

    /// A day row's add button box.
    fn add_box(&self, row: &DayRow) -> Rect {
        Rect::from_origin_size(
            Point::new(
                self.width - self.trailing_width(),
                row.rect.y0 + ROW_PADDING_Y,
            ),
            Size::new(ICON_BUTTON_BOX, ICON_BUTTON_BOX),
        )
    }

    /// A day row's copy button box, when there is one.
    fn copy_box(&self, row: &DayRow) -> Option<Rect> {
        self.has_copy.then(|| {
            let add = self.add_box(row);
            Rect::from_origin_size(
                Point::new(add.max_x() + ICON_BUTTON_GAP, add.y0),
                Size::new(ICON_BUTTON_BOX, ICON_BUTTON_BOX),
            )
        })
    }

    /// What a widget-local point lands on, if anything pressable.
    fn target_at(&self, at: Point) -> Option<Target> {
        for (day_index, row) in self.rows.iter().enumerate() {
            if !row.rect.contains(at) {
                continue;
            }
            if self.add_box(row).contains(at) {
                return Some(Target::Add(day_index));
            }
            if self.copy_box(row).is_some_and(|b| b.contains(at)) {
                return Some(Target::Copy(day_index));
            }
            if !row.enabled {
                return None;
            }
            for (range_index, range) in row.ranges.iter().enumerate() {
                if range.leaving {
                    continue;
                }
                let (start, end, remove) = self.range_boxes(row, range);
                if start.contains(at) {
                    return Some(Target::Field(day_index, range_index, TimeEdge::Start));
                }
                if end.contains(at) {
                    return Some(Target::Field(day_index, range_index, TimeEdge::End));
                }
                if remove.contains(at) {
                    return Some(Target::Remove(day_index, range_index));
                }
            }
            return None;
        }
        None
    }

    /// Report the edit `target` implies.
    fn activate(&mut self, ctx: &mut EventCtx, target: Target) {
        match target {
            Target::Field(day_index, range_index, edge) => {
                let day = DayKey::WEEK[day_index];
                let Some(range) = self.rows[day_index].ranges.get(range_index) else {
                    return;
                };
                let key = panel_key(day, &range.id, edge);
                // A press on the already-open field closes it, which is what a
                // trigger does everywhere else in this catalog.
                let next = (self.open_panel.as_deref() != Some(key.as_str())).then_some(key);
                (self.on_field_press)(ctx, next);
            }
            Target::Remove(day_index, range_index) => {
                let day = DayKey::WEEK[day_index];
                let Some(range) = self.rows[day_index].ranges.get(range_index) else {
                    return;
                };
                let next = remove_range(self.week.day(day), &range.id);
                let week = self.week.with_day(day, next);
                (self.on_week)(ctx, week);
            }
            Target::Add(day_index) => {
                let day = DayKey::WEEK[day_index];
                let id = self.mint_id(day);
                let next = add_range(self.week.day(day), || id);
                let week = self.week.with_day(day, next);
                (self.on_week)(ctx, week);
            }
            Target::Copy(day_index) => {
                if let Some(on_copy) = self.on_copy.as_mut() {
                    on_copy(ctx, DayKey::WEEK[day_index]);
                }
            }
        }
    }
}

/// The palette the scheduler paints from.
struct SchedulerColors {
    border: Color,
    ink: Color,
    muted: Color,
    muted_fill: Color,
}

fn resolve_colors(theme: Option<&Theme>) -> SchedulerColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            SchedulerColors {
                border: scheme.outline_variant,
                ink: scheme.on_surface,
                muted: scheme.on_surface_variant,
                muted_fill: scheme.surface_container_highest,
            }
        }
        None => SchedulerColors {
            border: FALLBACK_BORDER,
            ink: FALLBACK_FOREGROUND,
            muted: FALLBACK_MUTED,
            muted_fill: FALLBACK_MUTED_FILL,
        },
    }
}

/// The type-scale role a day's name takes its family from at layout.
const DAY_ROLE: ThemeTextType = ThemeTextType::LabelLarge;

/// The type-scale role the time ranges and the "Unavailable" line take their
/// family from at layout.
const BODY_ROLE: ThemeTextType = ThemeTextType::BodyMedium;

/// `font-medium` at `size`. The family here is the unthemed base; `layout`
/// shapes in [`DAY_ROLE`]'s family.
fn medium(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

/// A plain run at `size`. The family here is the unthemed base; `layout`
/// shapes in [`BODY_ROLE`]'s family.
fn plain(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

impl Widget for AvailabilitySchedulerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);

        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            SCHEDULER_MAX_WIDTH
        };
        self.width = SCHEDULER_MAX_WIDTH.min(available);

        let switch_bc = BoxConstraints::new(Size::ZERO, Size::new(LABEL_COLUMN, f64::INFINITY));
        let mut y = 0.0;
        for index in 0..self.rows.len() {
            let switch = self.switches[index].layout_child(ctx, &switch_bc);
            {
                let row = &mut self.rows[index];
                row.label
                    .layout_themed(ctx, &medium(style::TEXT_SM, colors.ink), DAY_ROLE);
                row.unavailable
                    .layout_themed(ctx, &plain(style::TEXT_SM, colors.muted), BODY_ROLE);
                let range_style = plain(style::TEXT_SM, colors.ink);
                for range in &mut row.ranges {
                    range.start.layout_themed(ctx, &range_style, BODY_ROLE);
                    range.end.layout_themed(ctx, &range_style, BODY_ROLE);
                }
            }

            // The range column's own height, graded by each range's presence so
            // the rows below glide up behind a removal.
            let row = &mut self.rows[index];
            let mut column = 0.0;
            let mut first = true;
            for range in &mut row.ranges {
                // The height follows the presence in both directions, so the
                // rows below glide rather than jumping; `paint` steps every
                // range's presence regardless of its height, so a range that
                // starts at zero still has a clock to grow on.
                let height = style::HEIGHT_MD * range.shown.clamp(0.0, 1.0);
                if height <= 0.0 {
                    range.rect = Rect::ZERO;
                    continue;
                }
                if !first {
                    column += RANGE_GAP;
                }
                first = false;
                range.rect = Rect::from_origin_size(
                    Point::new(LABEL_COLUMN + ROW_GAP, 0.0),
                    Size::new(0.0, height),
                );
                column += height;
            }
            let unavailable = if row.absent.is_visible() {
                row.unavailable.size().height.max(style::TEXT_SM)
            } else {
                0.0
            };
            let content = switch.height.max(column).max(unavailable);
            let height = ROW_PADDING_Y * 2.0 + content;
            row.rect = Rect::from_origin_size(Point::new(0.0, y), Size::new(self.width, height));

            // Now the ranges' real boxes, stacked from the row's own top.
            let mut range_y = y + ROW_PADDING_Y;
            for range in &mut row.ranges {
                if range.rect.height() <= 0.0 {
                    continue;
                }
                let height = range.rect.height();
                range.rect = Rect::from_origin_size(
                    Point::new(LABEL_COLUMN + ROW_GAP, range_y),
                    Size::new(0.0, height),
                );
                range_y += height + RANGE_GAP;
            }

            self.switches[index].set_origin(Point::new(0.0, y + ROW_PADDING_Y));
            y += height;
        }

        self.height = y;
        bc.constrain(Size::new(self.width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let origin = ctx.origin();
        let (colors, reduce) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };

        self.press
            .retarget(if self.armed.is_some() { 1.0 } else { 0.0 });
        let mut owes_frame = false;
        let mut owes_layout = false;
        if reduce {
            self.press.snap();
            for row in &mut self.rows {
                row.absent = row.absent.collapsed();
                for range in &mut row.ranges {
                    range.presence = range.presence.collapsed();
                }
            }
        } else {
            owes_frame |= self.press.advance(now);
        }
        for row in &mut self.rows {
            row.absent_shown = row.absent.advance(now);
            owes_frame |= row.absent.is_animating();
            for range in &mut row.ranges {
                range.shown = range.presence.advance(now);
                // Both directions are height-bearing, so either owes a
                // relayout rather than a bare repaint.
                owes_layout |= range.presence.is_animating();
            }
        }

        let press = self.press.value().clamp(0.0, 1.0);
        for index in 0..self.rows.len() {
            // `divide-y`: a hairline above every row but the first.
            if index > 0 {
                let y = origin.y + self.rows[index].rect.y0;
                let mut rule = BezPath::new();
                rule.move_to(Point::new(origin.x, y));
                rule.line_to(Point::new(origin.x + self.width, y));
                scene.stroke_path(
                    Point::ZERO,
                    &rule,
                    style::BORDER_WIDTH,
                    &Brush::Solid(colors.border),
                );
            }
            self.paint_row(index, origin, &colors, press, ctx, scene);
        }

        if owes_layout {
            ctx.request_layout();
        } else if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The switches own their own gestures.
        if route_event(&mut self.switches, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(target) = self.target_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed = Some(target);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.armed.is_none() {
                    let target = self.target_at(p.position);
                    if target != self.hovered {
                        self.hovered = target;
                        ctx.request_redraw();
                    }
                    if target.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                ctx.request_redraw();
                // Fire on up-inside only.
                if self.target_at(p.position) == Some(armed) {
                    self.activate(ctx, armed);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                // A `Cancel` arm clears internal flags only — never a callback.
                self.armed = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |node| node.set_label("Weekly availability"),
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    self.switches[index].semantics_child(ctx);
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(format!("Add time range to {}", row.day.label()));
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(switches);
}

impl AvailabilitySchedulerWidget {
    /// Paint day row `index`.
    fn paint_row(
        &mut self,
        index: usize,
        origin: Point,
        colors: &SchedulerColors,
        press: f64,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
    ) {
        // The switch pod first, then this widget's own chrome over it.
        self.switches[index].paint_child(ctx, scene);

        let row = &self.rows[index];
        let switch_size = self.switches[index].size();
        let label_at = Point::new(
            origin.x + switch_size.width + SWITCH_LABEL_GAP,
            origin.y
                + row.rect.y0
                + ROW_PADDING_Y
                + (switch_size.height - row.label.size().height) / 2.0,
        );
        row.label.paint(label_at, scene);

        if row.absent_shown > 0.0 {
            let at = Point::new(
                origin.x + LABEL_COLUMN + ROW_GAP,
                origin.y
                    + row.rect.y0
                    + ROW_PADDING_Y
                    + (style::HEIGHT_MD - row.unavailable.size().height) / 2.0
                    - RANGE_EXIT_RISE * (1.0 - row.absent_shown),
            );
            let size = row.unavailable.size();
            scene.push_layer(
                at,
                Size::new(size.width.max(1.0), size.height.max(1.0)),
                row.absent_shown.clamp(0.0, 1.0) as f32,
            );
            row.unavailable.paint(at, scene);
            scene.pop_layer();
        }

        let field_width = self.field_width();
        for (range_index, range) in row.ranges.iter().enumerate() {
            if range.rect.height() <= 0.0 {
                continue;
            }
            let shown = range.shown.clamp(0.0, 1.0);
            let rise = if range.leaving {
                -RANGE_EXIT_RISE * (1.0 - shown)
            } else {
                -RANGE_ENTER_RISE * (1.0 - shown)
            };
            let (start, end, remove) = self.range_boxes(row, range);
            let dy = origin.y + rise;
            scene.push_layer(
                Point::new(origin.x, dy + range.rect.y0),
                Size::new(self.width, style::HEIGHT_MD),
                shown as f32,
            );

            for (edge, rect, label) in [
                (TimeEdge::Start, start, &range.start),
                (TimeEdge::End, end, &range.end),
            ] {
                let at = Point::new(origin.x + rect.x0, dy + rect.y0);
                let size = Size::new(field_width, style::HEIGHT_MD);
                let target = Target::Field(index, range_index, edge);
                let scale = if self.armed == Some(target) {
                    press_scale(ICON_BUTTON_PRESS_SCALE, press)
                } else {
                    1.0
                };
                paint_pill(
                    at,
                    size,
                    scale,
                    colors.border,
                    (self.hovered == Some(target)).then_some(colors.muted_fill),
                    scene,
                );
                let text = label.size();
                label.paint(
                    Point::new(
                        at.x + style::PADDING_X_SM,
                        at.y + (size.height - text.height) / 2.0,
                    ),
                    scene,
                );
                draw_chevron(
                    Point::new(
                        at.x + size.width - style::PADDING_X_SM - TIME_FIELD_CHEVRON / 2.0,
                        at.y + size.height / 2.0,
                    ),
                    TIME_FIELD_CHEVRON,
                    colors.muted,
                    scene,
                );
            }

            // The `–` between the two fields.
            let dash_y = dy + start.y0 + style::HEIGHT_MD / 2.0;
            let dash_x = origin.x + start.max_x() + RANGE_INNER_GAP;
            let mut dash = BezPath::new();
            dash.move_to(Point::new(dash_x, dash_y));
            dash.line_to(Point::new(dash_x + DASH_WIDTH, dash_y));
            scene.stroke_path(
                Point::ZERO,
                &dash,
                style::BORDER_WIDTH * 1.5,
                &Brush::Solid(colors.muted),
            );

            let remove_target = Target::Remove(index, range_index);
            self.paint_icon_button(
                Point::new(origin.x + remove.x0, dy + remove.y0),
                remove_target,
                press,
                colors,
                scene,
            );
            draw_cross(
                Point::new(origin.x + remove.center().x, dy + remove.center().y),
                ICON_BUTTON_ICON * 0.6,
                colors.muted,
                scene,
            );
            scene.pop_layer();
        }

        // The trailing action column.
        let add = self.add_box(row);
        let add_at = Point::new(origin.x + add.x0, origin.y + add.y0);
        self.paint_icon_button(add_at, Target::Add(index), press, colors, scene);
        draw_plus(
            Point::new(
                add_at.x + ICON_BUTTON_BOX / 2.0,
                add_at.y + ICON_BUTTON_BOX / 2.0,
            ),
            ICON_BUTTON_ICON * 0.6,
            colors.muted,
            scene,
        );
        if let Some(copy) = self.copy_box(row) {
            let at = Point::new(origin.x + copy.x0, origin.y + copy.y0);
            self.paint_icon_button(at, Target::Copy(index), press, colors, scene);
            draw_copy(
                Point::new(at.x + ICON_BUTTON_BOX / 2.0, at.y + ICON_BUTTON_BOX / 2.0),
                ICON_BUTTON_ICON,
                colors.muted,
                scene,
            );
        }
    }

    /// An icon button's own chrome: the hover wash and the press shrink.
    fn paint_icon_button(
        &self,
        at: Point,
        target: Target,
        press: f64,
        colors: &SchedulerColors,
        scene: &mut dyn PaintScene,
    ) {
        let scale = if self.armed == Some(target) {
            press_scale(ICON_BUTTON_PRESS_SCALE, press)
        } else {
            1.0
        };
        if self.hovered != Some(target) {
            return;
        }
        let edge = ICON_BUTTON_BOX * scale;
        let inset = (ICON_BUTTON_BOX - edge) / 2.0;
        scene.fill_rounded_rect(
            Point::new(at.x + inset, at.y + inset),
            Size::new(edge, edge),
            style::RADIUS_LG,
            colors.muted_fill,
        );
    }
}

/// Paint a bordered pill, shrunk about its own centre by `scale`.
fn paint_pill(
    at: Point,
    size: Size,
    scale: f64,
    border: Color,
    wash: Option<Color>,
    scene: &mut dyn PaintScene,
) {
    let box_at = Point::new(
        at.x + size.width * (1.0 - scale) / 2.0,
        at.y + size.height * (1.0 - scale) / 2.0,
    );
    let box_size = Size::new(size.width * scale, size.height * scale);
    let radius = style::resolve_radius(style::RADIUS_CONTROL, box_size.width, box_size.height);
    if let Some(wash) = wash {
        scene.fill_rounded_rect(box_at, box_size, radius, wash);
    }
    let half = style::BORDER_WIDTH / 2.0;
    let frame = RoundedRect::from_rect(
        Rect::from_origin_size(Point::ORIGIN, box_size).inset(-half),
        (radius - half).max(0.0),
    );
    scene.stroke_path(
        box_at,
        &Shape::to_path(&frame, style::PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(border),
    );
}

/// The lucide stroke width, in viewBox units.
const ICON_STROKE_VIEWBOX: f64 = 2.0;

/// The stroke width a `size`-square glyph is drawn at.
fn icon_stroke(size: f64) -> f64 {
    ICON_STROKE_VIEWBOX * size / 24.0
}

/// Lucide's `ChevronDown`, centred on `centre`.
fn draw_chevron(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let half = size / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - half * 0.7, centre.y - half * 0.3));
    path.line_to(Point::new(centre.x, centre.y + half * 0.4));
    path.line_to(Point::new(centre.x + half * 0.7, centre.y - half * 0.3));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

/// Lucide's `X`, centred on `centre`.
fn draw_cross(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let arm = size / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - arm, centre.y - arm));
    path.line_to(Point::new(centre.x + arm, centre.y + arm));
    path.move_to(Point::new(centre.x + arm, centre.y - arm));
    path.line_to(Point::new(centre.x - arm, centre.y + arm));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

/// Lucide's `Plus`, centred on `centre`.
fn draw_plus(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let arm = size / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - arm, centre.y));
    path.line_to(Point::new(centre.x + arm, centre.y));
    path.move_to(Point::new(centre.x, centre.y - arm));
    path.line_to(Point::new(centre.x, centre.y + arm));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

/// Lucide's `Copy` — two offset squares — centred on `centre`.
fn draw_copy(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let edge = size * 0.62;
    let offset = size * 0.18;
    let stroke = icon_stroke(size);
    for (dx, dy) in [(offset, -offset), (-offset, offset)] {
        let rect = RoundedRect::new(
            centre.x + dx - edge / 2.0,
            centre.y + dy - edge / 2.0,
            centre.x + dx + edge / 2.0,
            centre.y + dy + edge / 2.0,
            size * 0.12,
        );
        scene.stroke_path(
            Point::ZERO,
            &Shape::to_path(&rect, style::PATH_TOLERANCE),
            stroke,
            &Brush::Solid(color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    /// Records the ops these tests assert on.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        glyphs: Vec<Point>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    /// What the app state records.
    #[derive(Default)]
    struct App {
        week: WeekAvailability,
        panels: Vec<Option<String>>,
        copies: Vec<DayKey>,
    }

    const WIDTH: f64 = 576.0;

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn pointer(phase: PointerPhase, at: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: at,
            button: PointerButton::Primary,
        })
    }

    fn view(week: WeekAvailability) -> AvailabilitySchedulerView<App> {
        availability_scheduler::<App, _>(week, |s: &mut App, next| s.week = next)
            .on_panel_open_change(|s: &mut App, key| s.panels.push(key))
            .on_copy_request(|s: &mut App, day| s.copies.push(day))
    }

    /// A widget plus the rebuild an app performs around it.
    struct Bare {
        widget: AvailabilitySchedulerWidget,
        view: AvailabilitySchedulerView<App>,
        state: App,
        counter: u64,
        theme: Theme,
    }

    impl Bare {
        fn new(week: WeekAvailability) -> Self {
            Self::with(view(week))
        }

        fn with(view: AvailabilitySchedulerView<App>) -> Self {
            let mut counter = 0u64;
            let widget = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut bare = Bare {
                widget,
                view,
                state: App::default(),
                counter,
                theme: crate::theme(),
            };
            bare.state.week = bare.view.value.clone();
            bare.layout();
            bare
        }

        fn layout(&mut self) -> Size {
            let mut tcx = TextContext::new();
            let lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let mut lctx = lctx.with_theme(&self.theme as &dyn Any);
            self.widget
                .layout(&mut lctx, &BoxConstraints::loose(Size::new(WIDTH, 2_000.0)))
        }

        fn rebuild(&mut self, next: AvailabilitySchedulerView<App>) {
            let mut ctx = BuildCtx::new(&mut self.counter);
            View::<App>::rebuild(&next, &self.view, &mut self.widget, &mut ctx);
            self.view = next;
            self.layout();
        }

        /// Feed the app's own week straight back down.
        fn round_trip(&mut self) {
            let mut next = view(self.state.week.clone());
            next.open_panel = self.view.open_panel.clone();
            self.rebuild(next);
        }

        fn paint_at(&mut self, millis: f64) -> Recorder {
            let mut rec = Recorder::default();
            let size = Size::new(self.widget.width, self.widget.height.max(1.0));
            let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(millis))
                .with_theme(&self.theme as &dyn Any);
            self.widget.paint(&mut ctx, &mut rec);
            rec
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventResult {
            let size = Size::new(self.widget.width, self.widget.height.max(1.0));
            let state: &mut dyn Any = &mut self.state;
            let mut ctx = EventCtx::new(state, Point::ZERO, size);
            self.widget.event(&mut ctx, event)
        }

        fn click(&mut self, at: Point) {
            self.dispatch(&pointer(PointerPhase::Down, at));
            self.dispatch(&pointer(PointerPhase::Up, at));
        }
    }

    // ---- Time helpers -------------------------------------------------------

    #[test]
    fn the_time_helpers_round_trip_and_clamp() {
        assert_eq!(to_minutes("09:00"), 540);
        assert_eq!(to_minutes("00:00"), 0);
        assert_eq!(to_minutes("23:59"), 1_439);
        // A malformed value reads as midnight rather than panicking.
        assert_eq!(to_minutes("nonsense"), 0);
        assert_eq!(to_minutes("aa:bb"), 0);

        assert_eq!(to_value(540), "09:00");
        assert_eq!(to_value(0), "00:00");
        // Past the end of the day it clamps rather than wrapping.
        assert_eq!(to_value(MINUTES_PER_DAY), "23:59");
        assert_eq!(to_value(u32::MAX), "23:59");

        for minutes in [0, 1, 540, 719, 720, 1_439] {
            assert_eq!(to_minutes(&to_value(minutes)), minutes);
        }
    }

    #[test]
    fn the_twelve_hour_label_names_noon_and_midnight_correctly() {
        assert_eq!(label_12("00:00"), "12:00 AM");
        assert_eq!(label_12("00:30"), "12:30 AM");
        assert_eq!(label_12("09:00"), "9:00 AM");
        assert_eq!(label_12("11:59"), "11:59 AM");
        assert_eq!(label_12("12:00"), "12:00 PM");
        assert_eq!(label_12("13:05"), "1:05 PM");
        assert_eq!(label_12("23:30"), "11:30 PM");
    }

    #[test]
    fn build_options_walks_the_day_at_the_requested_step() {
        let half_hourly = build_options(30);
        assert_eq!(half_hourly.len(), 48);
        assert_eq!(half_hourly[0], "00:00");
        assert_eq!(half_hourly[1], "00:30");
        assert_eq!(half_hourly[47], "23:30");

        assert_eq!(build_options(60).len(), 24);
        assert_eq!(build_options(720).len(), 2);
        // A zero step would be an unbounded list; it floors at a minute.
        assert_eq!(build_options(0).len(), MINUTES_PER_DAY as usize);
    }

    // ---- clamp_range: the range's own start/end semantics ---------------------

    #[test]
    fn a_valid_pair_is_left_exactly_alone_even_off_grid() {
        let options = build_options(720);
        // 17:00 is not a generated option at a twelve-hour step, and must not
        // be rewritten while the pair is still valid.
        assert_eq!(
            clamp_range("09:00", "17:00", &options, TimeEdge::Start),
            ("09:00".to_string(), "17:00".to_string())
        );
        assert_eq!(
            clamp_range("09:00", "17:00", &options, TimeEdge::End),
            ("09:00".to_string(), "17:00".to_string())
        );
    }

    #[test]
    fn changing_the_start_past_the_end_moves_the_end_forward() {
        let options = build_options(30);
        // The just-chosen start stays; the end moves to the next option.
        let (start, end) = clamp_range("18:00", "17:00", &options, TimeEdge::Start);
        assert_eq!(start, "18:00");
        assert_eq!(end, "18:30");
        // Equal endpoints are not a positive range either.
        let (start, end) = clamp_range("09:00", "09:00", &options, TimeEdge::Start);
        assert_eq!((start.as_str(), end.as_str()), ("09:00", "09:30"));
    }

    #[test]
    fn changing_the_end_before_the_start_moves_the_start_back() {
        let options = build_options(30);
        // The just-chosen end stays; the start moves to the previous option.
        let (start, end) = clamp_range("18:00", "09:00", &options, TimeEdge::End);
        assert_eq!(end, "09:00");
        assert_eq!(start, "08:30");
    }

    #[test]
    fn the_two_impossible_endpoints_fall_back_to_a_neighbouring_pair() {
        let options = build_options(30);
        // Midnight cannot end a positive same-day range: take the first pair.
        let (start, end) = clamp_range("09:00", "00:00", &options, TimeEdge::End);
        assert_eq!((start.as_str(), end.as_str()), ("00:00", "00:30"));
        // The last slot cannot start one: take the final pair.
        let (start, end) = clamp_range("23:30", "09:00", &options, TimeEdge::Start);
        assert_eq!((start.as_str(), end.as_str()), ("23:00", "23:30"));
        // An empty option list is left untouched rather than panicking.
        assert_eq!(
            clamp_range("09:00", "09:00", &[], TimeEdge::Start),
            ("09:00".to_string(), "09:00".to_string())
        );
    }

    #[test]
    fn an_off_grid_endpoint_snaps_to_its_nearest_option() {
        let options = build_options(60);
        // 09:20 is not an option; the nearest is 09:00, and the end moves to
        // the next one after it.
        let (start, end) = clamp_range("09:20", "08:00", &options, TimeEdge::Start);
        assert_eq!((start.as_str(), end.as_str()), ("09:00", "10:00"));
    }

    // ---- The option lists ----------------------------------------------------

    #[test]
    fn the_option_lists_are_bounded_by_the_other_endpoint() {
        let options = build_options(60);
        let starts = start_options(&options, "12:00", Some("09:00"));
        assert!(starts.iter().all(|o| to_minutes(o) < 720));
        assert_eq!(starts.last().unwrap(), "11:00");

        let ends = end_options(&options, "09:00", Some("17:00"));
        assert!(ends.iter().all(|o| to_minutes(o) > 540));
        assert_eq!(ends.first().unwrap(), "10:00");
    }

    #[test]
    fn a_current_value_the_filter_dropped_stays_selectable_and_in_order() {
        let options = build_options(720);
        // 17:00 survives the end filter even though it is not generated...
        let ends = end_options(&options, "09:00", Some("17:00"));
        assert!(ends.iter().any(|o| o == "17:00"));
        assert!(
            ends.windows(2)
                .all(|w| to_minutes(&w[0]) < to_minutes(&w[1]))
        );
        // ...and so does an off-grid start.
        let starts = start_options(&options, "17:00", Some("09:00"));
        assert!(starts.iter().any(|o| o == "09:00"));
        assert!(
            starts
                .windows(2)
                .all(|w| to_minutes(&w[0]) < to_minutes(&w[1]))
        );
    }

    #[test]
    fn the_two_recovery_lists_keep_an_invalid_row_escapable() {
        let options = build_options(30);
        // No option is earlier than midnight, so midnight itself is offered —
        // choosing it moves the end forward through `clamp_range`.
        let starts = start_options(&options, "00:00", None);
        assert_eq!(starts, vec!["00:00".to_string()]);
        // No option is later than the last slot, so the last slot is offered.
        let ends = end_options(&options, "23:30", None);
        assert_eq!(ends, vec!["23:30".to_string()]);
    }

    // ---- The week's own edits -------------------------------------------------

    #[test]
    fn the_default_week_is_weekdays_nine_to_five() {
        let week = default_week();
        for day in DayKey::WEEK {
            let state = week.day(day);
            assert_eq!(state.ranges.len(), 1);
            assert_eq!(state.ranges[0].start, DEFAULT_START);
            assert_eq!(state.ranges[0].end, DEFAULT_END);
            let weekend = matches!(day, DayKey::Sat | DayKey::Sun);
            assert_eq!(state.enabled, !weekend, "{} enabled", day.label());
            // Fixed ids, so a first render agrees with itself.
            assert_eq!(state.ranges[0].id, format!("{}-0", day.key()));
        }
    }

    #[test]
    fn enabling_a_day_with_no_hours_gives_it_the_default_window() {
        let empty = DayAvailability::default();
        let on = set_enabled(&empty, true, || "mon-n1".to_string());
        assert!(on.enabled);
        assert_eq!(on.ranges.len(), 1);
        assert_eq!(on.ranges[0].start, DEFAULT_START);
        assert_eq!(on.ranges[0].end, DEFAULT_END);

        // A day that already has hours keeps them, on or off.
        let kept = set_enabled(&on, false, || unreachable!("no id is minted"));
        assert!(!kept.enabled);
        assert_eq!(kept.ranges, on.ranges);
    }

    #[test]
    fn adding_a_range_starts_an_hour_after_the_last_and_runs_an_hour() {
        let day = DayAvailability {
            enabled: true,
            ranges: vec![TimeRange::new("a", "09:00", "17:00")],
        };
        let next = add_range(&day, || "b".to_string());
        assert_eq!(next.ranges.len(), 2);
        assert_eq!(next.ranges[1].start, "18:00");
        assert_eq!(next.ranges[1].end, "19:00");

        // It never starts later than an hour before midnight.
        let late = DayAvailability {
            enabled: true,
            ranges: vec![TimeRange::new("a", "22:00", "23:30")],
        };
        let next = add_range(&late, || "b".to_string());
        assert_eq!(next.ranges[1].start, "23:00");
        assert_eq!(next.ranges[1].end, "23:59");

        // Adding to an off day switches it on, at nine.
        let off = DayAvailability::default();
        let next = add_range(&off, || "a".to_string());
        assert!(next.enabled);
        assert_eq!(next.ranges[0].start, DEFAULT_START);
    }

    #[test]
    fn removing_the_last_range_marks_the_day_unavailable() {
        let day = DayAvailability {
            enabled: true,
            ranges: vec![
                TimeRange::new("a", "09:00", "12:00"),
                TimeRange::new("b", "13:00", "17:00"),
            ],
        };
        let one = remove_range(&day, "a");
        assert!(one.enabled);
        assert_eq!(one.ranges.len(), 1);
        assert_eq!(one.ranges[0].id, "b");

        let none = remove_range(&one, "b");
        assert!(!none.enabled, "an hourless day is unavailable");
        assert!(none.ranges.is_empty());

        // Removing an id the day does not have changes nothing but the flag.
        let missing = remove_range(&day, "zz");
        assert_eq!(missing.ranges, day.ranges);
    }

    #[test]
    fn copying_a_day_clones_its_hours_under_fresh_ids() {
        let week = default_week();
        let mut counter = 0;
        let next = copy_day(&week, DayKey::Mon, &[DayKey::Sat, DayKey::Sun], |day| {
            counter += 1;
            format!("{}-c{counter}", day.key())
        });
        for target in [DayKey::Sat, DayKey::Sun] {
            let copied = next.day(target);
            assert!(copied.enabled, "the source's own flag travels");
            assert_eq!(copied.ranges.len(), 1);
            assert_eq!(copied.ranges[0].start, DEFAULT_START);
            assert_ne!(
                copied.ranges[0].id,
                week.day(DayKey::Mon).ranges[0].id,
                "with a fresh id"
            );
        }
        // Untouched days are untouched.
        assert_eq!(next.day(DayKey::Tue), week.day(DayKey::Tue));
    }

    #[test]
    fn a_panel_key_names_one_field_week_wide() {
        assert_eq!(
            panel_key(DayKey::Mon, "mon-0", TimeEdge::Start),
            "mon:mon-0:start"
        );
        assert_ne!(
            panel_key(DayKey::Mon, "x", TimeEdge::Start),
            panel_key(DayKey::Mon, "x", TimeEdge::End)
        );
        // Range ids are unique only within a day, which is why the day is part
        // of the name.
        assert_ne!(
            panel_key(DayKey::Mon, "0", TimeEdge::Start),
            panel_key(DayKey::Tue, "0", TimeEdge::Start)
        );
    }

    #[test]
    fn live_panels_names_every_field_on_screen_and_nothing_else() {
        let scheduler = view(default_week());
        let live = scheduler.live_panels();
        // Five weekdays, one range each, two fields per range.
        assert_eq!(live.len(), 10);
        assert!(live.contains(&panel_key(DayKey::Mon, "mon-0", TimeEdge::Start)));
        assert!(
            !live.contains(&panel_key(DayKey::Sat, "sat-0", TimeEdge::Start)),
            "a switched-off day shows no fields"
        );
    }

    // ---- The widget -----------------------------------------------------------

    #[test]
    fn the_scheduler_lays_out_seven_rows_and_one_switch_each() {
        let mut bare = Bare::new(default_week());
        let size = bare.layout();
        assert_eq!(size.width, SCHEDULER_MAX_WIDTH);
        assert_eq!(bare.widget.rows.len(), 7);
        assert_eq!(bare.widget.switches.len(), 7);
        let mut previous = -1.0;
        for (index, row) in bare.widget.rows.iter().enumerate() {
            assert!(row.rect.y0 > previous);
            previous = row.rect.y0;
            assert_eq!(bare.widget.switches[index].origin().x, 0.0);
        }
        assert!((size.height - bare.widget.rows[6].rect.max_y()).abs() < 1e-6);
    }

    #[test]
    fn an_enabled_day_shows_its_ranges_and_an_off_day_shows_the_word() {
        let mut bare = Bare::new(default_week());
        bare.paint_at(0.0);
        let monday = &bare.widget.rows[DayKey::Mon.index()];
        assert_eq!(monday.ranges.len(), 1);
        assert!(monday.ranges[0].rect.height() > 0.0);
        assert!(monday.absent_shown < 1e-6, "Monday is available");

        let saturday = &bare.widget.rows[DayKey::Sat.index()];
        assert!(
            saturday.absent_shown > 0.0,
            "Saturday says it is unavailable"
        );
    }

    #[test]
    fn pressing_a_time_field_reports_its_panel_key_and_pressing_it_again_closes_it() {
        let mut bare = Bare::new(default_week());
        let monday = &bare.widget.rows[DayKey::Mon.index()];
        let (start, _, _) = bare.widget.range_boxes(monday, &monday.ranges[0]);
        bare.click(start.center());
        assert_eq!(
            bare.state.panels,
            vec![Some(panel_key(DayKey::Mon, "mon-0", TimeEdge::Start))]
        );

        // With that panel open, pressing the same field closes it.
        let mut next = view(bare.state.week.clone());
        next.open_panel = bare.state.panels[0].clone();
        bare.rebuild(next);
        bare.click(start.center());
        assert_eq!(bare.state.panels[1], None);
    }

    #[test]
    fn a_panel_key_the_week_no_longer_shows_is_reported_closed() {
        let mut bare = Bare::new(default_week());
        let key = panel_key(DayKey::Mon, "mon-0", TimeEdge::Start);
        let mut open = view(default_week());
        open.open_panel = Some(key.clone());
        bare.rebuild(open);
        assert_eq!(bare.widget.open_panel.as_deref(), Some(key.as_str()));

        // Switching Monday off takes its fields off screen.
        let mut week = default_week();
        week.set_day(
            DayKey::Mon,
            DayAvailability {
                enabled: false,
                ..week.day(DayKey::Mon).clone()
            },
        );
        let mut closed = view(week);
        closed.open_panel = Some(key);
        bare.rebuild(closed);
        assert_eq!(bare.widget.open_panel, None);
    }

    #[test]
    fn the_add_button_appends_a_range_and_the_remove_button_drops_one() {
        let mut bare = Bare::new(default_week());
        let add = bare.widget.add_box(&bare.widget.rows[DayKey::Mon.index()]);
        bare.click(add.center());
        assert_eq!(bare.state.week.day(DayKey::Mon).ranges.len(), 2);
        assert_eq!(bare.state.week.day(DayKey::Mon).ranges[1].start, "18:00");
        bare.round_trip();
        assert_eq!(bare.widget.rows[DayKey::Mon.index()].ranges.len(), 2);

        // And the second range's remove button drops it again.
        let monday = &bare.widget.rows[DayKey::Mon.index()];
        let (_, _, remove) = bare.widget.range_boxes(monday, &monday.ranges[1]);
        bare.click(remove.center());
        assert_eq!(bare.state.week.day(DayKey::Mon).ranges.len(), 1);
    }

    #[test]
    fn two_added_ranges_never_share_an_id() {
        let mut bare = Bare::new(default_week());
        for _ in 0..2 {
            let add = bare.widget.add_box(&bare.widget.rows[DayKey::Mon.index()]);
            bare.click(add.center());
            bare.round_trip();
        }
        let ids: Vec<&str> = bare
            .state
            .week
            .day(DayKey::Mon)
            .ranges
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids.len(), 3);
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "ids collided: {ids:?}");
    }

    #[test]
    fn a_removed_range_is_kept_mounted_through_its_exit_then_dropped() {
        let mut bare = Bare::new(default_week());
        let add = bare.widget.add_box(&bare.widget.rows[DayKey::Mon.index()]);
        bare.click(add.center());
        bare.round_trip();
        // The new range enters on a ramp, so it takes a settled pass before it
        // occupies its own row.
        bare.paint_at(0.0);
        bare.paint_at(10_000.0);
        let tall = bare.layout().height;

        let monday = &bare.widget.rows[DayKey::Mon.index()];
        assert!(
            monday.ranges[1].rect.height() > 0.0,
            "the new range settled in"
        );
        let (_, _, remove) = bare.widget.range_boxes(monday, &monday.ranges[1]);
        bare.click(remove.center());
        bare.round_trip();
        assert_eq!(
            bare.widget.rows[DayKey::Mon.index()].ranges.len(),
            2,
            "still mounted for its exit"
        );
        assert!(bare.widget.rows[DayKey::Mon.index()].ranges[1].leaving);

        bare.paint_at(10_001.0);
        bare.paint_at(20_000.0);
        bare.round_trip();
        assert_eq!(bare.widget.rows[DayKey::Mon.index()].ranges.len(), 1);
        assert!(bare.layout().height < tall, "the rows below glide back up");
    }

    #[test]
    fn the_copy_affordance_reports_its_day_and_is_absent_without_a_handler() {
        let mut bare = Bare::new(default_week());
        let copy = bare
            .widget
            .copy_box(&bare.widget.rows[DayKey::Wed.index()])
            .expect("a copy button");
        bare.click(copy.center());
        assert_eq!(bare.state.copies, vec![DayKey::Wed]);

        let plain: AvailabilitySchedulerView<App> =
            availability_scheduler(default_week(), |s: &mut App, next| s.week = next);
        let mut bare = Bare::with(plain);
        assert!(!bare.widget.has_copy);
        assert!(bare.widget.copy_box(&bare.widget.rows[0]).is_none());
        // ...and the space it would have taken goes back to the range column.
        assert_eq!(bare.widget.trailing_width(), ICON_BUTTON_BOX);
        let _ = bare.layout();
    }

    #[test]
    fn an_off_days_hours_are_neither_painted_nor_pressable() {
        let mut bare = Bare::new(default_week());
        let saturday = &bare.widget.rows[DayKey::Sat.index()];
        // The model still carries Saturday's window; the row does not show it.
        assert_eq!(saturday.ranges.len(), 1);
        assert_eq!(saturday.ranges[0].rect.height(), 0.0);

        // Pressing where a field would be reports nothing.
        let at = Point::new(
            LABEL_COLUMN + ROW_GAP + 10.0,
            saturday.rect.y0 + ROW_PADDING_Y + style::HEIGHT_MD / 2.0,
        );
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Down, at)),
            EventResult::Ignored
        );
        assert!(bare.state.panels.is_empty());
    }

    #[test]
    fn a_non_primary_press_never_arms_anything_and_a_cancel_disarms() {
        let mut bare = Bare::new(default_week());
        let add = bare.widget.add_box(&bare.widget.rows[0]);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: add.center(),
            button: PointerButton::Secondary,
        });
        assert_eq!(bare.dispatch(&secondary), EventResult::Ignored);
        assert!(bare.widget.armed.is_none());

        bare.dispatch(&pointer(PointerPhase::Down, add.center()));
        assert_eq!(bare.widget.armed, Some(Target::Add(0)));
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Cancel, add.center())),
            EventResult::Handled
        );
        assert!(bare.widget.armed.is_none());
        assert_eq!(bare.state.week, default_week(), "nothing was reported");
    }

    #[test]
    fn a_press_that_wanders_off_its_affordance_reports_nothing() {
        let mut bare = Bare::new(default_week());
        let add = bare.widget.add_box(&bare.widget.rows[0]);
        bare.dispatch(&pointer(PointerPhase::Down, add.center()));
        bare.dispatch(&pointer(PointerPhase::Up, Point::new(2.0, 2.0)));
        assert_eq!(bare.state.week, default_week());
    }

    // ---- Paint ---------------------------------------------------------------

    #[test]
    fn every_row_but_the_first_is_separated_by_a_hairline() {
        let mut bare = Bare::new(default_week());
        let rec = bare.paint_at(0.0);
        let theme = crate::theme();
        let border = theme.scheme().outline_variant;
        let rules = rec
            .strokes
            .iter()
            .filter(|(bbox, w, c)| {
                *w == style::BORDER_WIDTH && *c == border && bbox.height() < 1e-9
            })
            .count();
        assert_eq!(rules, 6, "`divide-y` draws six rules for seven rows");
    }

    #[test]
    fn each_visible_range_paints_two_pills_and_a_dash() {
        let mut bare = Bare::new(default_week());
        let rec = bare.paint_at(0.0);
        // Five available days, two pills each.
        let pills = rec
            .strokes
            .iter()
            .filter(|(bbox, w, _)| {
                // A pill's stroked frame is inset by half its own width on
                // each side, so its bounding box is one px shorter than the box.
                *w == style::BORDER_WIDTH
                    && (bbox.height() - (style::HEIGHT_MD - style::BORDER_WIDTH)).abs() < 1e-6
            })
            .count();
        assert_eq!(pills, 10, "two per available day, none on the weekend");
        // ...and a thicker dash between each pair.
        let dashes = rec
            .strokes
            .iter()
            .filter(|(_, w, _)| (*w - style::BORDER_WIDTH * 1.5).abs() < 1e-9)
            .count();
        assert_eq!(dashes, 5);
    }

    #[test]
    fn reduced_motion_lands_a_removal_at_once() {
        let mut bare = Bare::new(default_week());
        bare.theme.motion.reduce_motion = true;
        let mut week = default_week();
        week.set_day(
            DayKey::Mon,
            DayAvailability {
                enabled: false,
                ranges: Vec::new(),
            },
        );
        let mut next = view(week);
        next.open_panel = None;
        bare.rebuild(next);
        bare.paint_at(0.0);
        assert!(
            !bare.widget.rows[DayKey::Mon.index()].ranges[0]
                .presence
                .is_visible(),
            "the removed range is gone on the frame it left"
        );
    }

    // ---- Typeface: day names, ranges and "Unavailable" follow the theme -----

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(600.0, 900.0);

    /// The default week: five days with a range each, and two unavailable.
    fn probe_view(_: &mut ()) -> AvailabilitySchedulerView<()> {
        availability_scheduler::<(), _>(default_week(), |_: &mut (), _| {})
    }

    #[test]
    fn the_schedule_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the schedule's text", probe_view, PROBE_WINDOW);
        let runs = Probe::new(probe_view, PROBE_WINDOW, crate::theme()).frame();
        assert_eq!(
            runs.len(),
            19,
            "seven day names, five ranges' two ends and two \"Unavailable\" lines"
        );
    }

    #[test]
    fn the_schedule_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the schedule's text", probe_view, PROBE_WINDOW);
    }
}
