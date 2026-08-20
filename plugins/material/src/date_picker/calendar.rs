// Ported from `material_3_expressive` v1.0.8's embeddable calendar (MIT, © 2026
// Paa Developments;
// `tmp/material_3_expressive/lib/components/date_pickers/m3e_calendar_date_picker.dart`
// and `components/{m3e_day_cell, m3e_day_picker, m3e_month_picker,
// m3e_year_picker, m3e_date_picker_mode_toggle}.dart`, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (each documented in the module docs below): the whole
// calendar is one self-painting widget rather than a `PageView` of nested
// `StatefulWidget`s; the month page slide is driven off the rebuild diff rather
// than off a `PageController`; the widget paints no container of its own; and
// there is no keyboard paging, since this widget claims no focus.

//! The embeddable M3E calendar: month sub-header, 7-column day grid, and the
//! scrolling year grid the sub-header toggles to.
//!
//! [`calendar_date_picker`] is a **controlled** view over one
//! [`DatePickerState`] — see [`mod@super`]'s controlled note. Every gesture
//! (a day cell, a year cell, the prev/next chevrons, the sub-header toggle)
//! computes the next state with one of that type's pure transitions and reports
//! it through `on_change`; the widget writes nothing durable of its own.
//!
//! # One self-painting widget, no child pods
//!
//! The reference nests four widgets (`M3ECalendarDatePicker` → `M3EMonthPicker`
//! → `M3EDayPicker` → `M3EDayCell`, plus `M3EYearPicker`), each a
//! `StatefulWidget` or a `GridView.builder` item. This port is a single
//! [`Widget`]: a month has up to 42 cells and a decade of years up to 30 visible
//! ones, and a `ChildPod` per cell would pay a build/layout/paint/teardown
//! round trip for what is a filled circle plus one shaped number. The day
//! numbers `1..=31`, the seven weekday initials, the month/year label and the
//! visible year labels are shaped once into retained [`TextLayout`]s and
//! re-brushed at paint time (the idiom [`crate::badge`]'s `LabelRun`
//! establishes), so a repaint re-shapes nothing.
//!
//! The consequence a reader should know about: hit-testing, hover, and the
//! accessibility nodes are all derived from the geometry recorded during
//! `layout` rather than from pods. Every cell still contributes its own
//! semantics node with its own bounds (`docs/CODE_STANDARDS.md`'s Semantics
//! Conventions allow a widget to contribute several), so a screen reader sees
//! the grid, not one opaque box.
//!
//! # Month paging: the slide follows the rebuild diff
//!
//! The reference pages with a `PageView` whose `PageController.animateToPage`
//! both *is* the state change and drives the animation. Here the state change
//! is the app's (`on_change` → the app's own `displayed_month`), so the slide is
//! started from [`View::rebuild`] when the incoming
//! [`DatePickerState::displayed_month`] differs from the previous one: the
//! outgoing month slides out and the incoming one slides in over
//! [`MONTH_SCROLL_DURATION`], in the direction of the month delta. A jump of
//! more than one month (a year-cell tap, an app-driven set) slides once, not
//! once per month.
//!
//! `Theme.motion.reduce_motion` skips the slide entirely.
//!
//! # The year grid scrolls; the day grid does not
//!
//! The year grid is capped at [`MAX_DAY_PICKER_HEIGHT`](super::MAX_DAY_PICKER_HEIGHT)
//! ([`grid::year_grid_height`]) and scrolls inside that viewport when the
//! `first_date..=last_date` span needs more — wheel/trackpad
//! ([`InputEvent::Scroll`]) and drag, the latter armed only past
//! [`frust::input::TOUCH_SLOP`] so a tap on a year cell is never stolen by a
//! stray pixel of movement. Entering the year sub-view seeds the offset at the
//! selected year's row, the reference's `_M3EYearPickerState._initialOffset`.
//!
//! # Container: content, not chrome
//!
//! The reference's inline calendar wraps itself in a `surfaceContainerHigh`
//! rounded `Container` and its dialog calendar (`expandToFit: true`) does not.
//! This port always paints the bare content — the dialog panel
//! ([`crate::overlay::modal`]) already fills and rounds it, and a standalone
//! calendar is wrapped by the app (`card(..)`, or any filled container) the same
//! way [`crate::search`]'s view content leaves its chrome to its host. Nothing
//! here paints a background.

use std::collections::HashMap;
use std::rc::Rc;

use frust::authoring::text::{FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, ErasedArgCallback, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, ScrollDelta, SemanticsCtx,
    TypedArgCallback, View, Widget, erase_callback_arg,
};
use frust::input::TOUCH_SLOP;
use frust::{AnimationController, Theme};
use kurbo::{Circle, Point, Rect, Shape, Size};
use peniko::{Brush, Color};

use super::date::{DateRange, MaterialDate};
use super::grid::{self, MonthGrid};
use super::{
    ARROW_ICON_SIZE, ARROW_PADDING, CALENDAR_WIDTH, DAY_GRID_TOP_PADDING, DAY_ROW_HEIGHT, DAY_SIZE,
    DAYS_PER_WEEK, DISABLED_DAY_OPACITY, DatePickerMode, DatePickerState, DatePickerStrings,
    MONTH_SCROLL_DURATION, RANGE_HIGHLIGHT_ALPHA, SUB_HEADER_HEIGHT, SUB_HEADER_START_INSET,
    WEEKDAY_ROW_HEIGHT, YEAR_COLUMN_COUNT, YEAR_GRID_PADDING, YEAR_ROW_HEIGHT, YEAR_ROW_SPACING,
};
use crate::dropdown::Glyph;
use crate::interaction::{HapticSignal, MaterialHaptics};
use crate::press::presses;
use crate::state_layer::StateLayer;
use crate::tokens::MaterialMotion;

/// The day number's type role (M3 `bodyMedium`, `M3EDatePickerTheme.dayStyle`).
/// Hardcoded rather than read off a live [`Theme`]'s type scale for the reason
/// every catalog-owned run in this crate is: text is shaped in the *layout*
/// pass, which is handed no theme.
const DAY_TEXT_SIZE: f32 = 14.0;
/// The weekday initial's type role (M3 `bodySmall`, `weekdayStyle`).
const WEEKDAY_TEXT_SIZE: f32 = 12.0;
/// The sub-header label's type role (M3 `titleSmall`, the reference's
/// `theme.typeScale.titleSmall`).
const SUB_HEADER_TEXT_SIZE: f32 = 14.0;
/// The year cell's type role (M3 `titleMedium`, `yearStyle`).
const YEAR_TEXT_SIZE: f32 = 16.0;
/// The ink every retained run is *shaped* with; each is re-brushed with its
/// resolved role at paint time so a recolor never misses the shape cache (the
/// convention [`crate::text_field`]'s `SHAPING_INK` records).
const SHAPING_INK: Color = Color::BLACK;

/// The today ring's stroke width, in logical px — the reference's
/// `Border.all(color: todayColor)`, whose Flutter default is 1.0.
const TODAY_RING_WIDTH: f64 = 1.0;
/// Curve-flattening tolerance for the day/year circles.
const PATH_TOLERANCE: f64 = 0.1;
/// Logical px scrolled per wheel line in the year grid — one year row plus its
/// spacing, so a notch advances exactly one row.
const YEAR_WHEEL_LINE: f64 = YEAR_ROW_HEIGHT + YEAR_ROW_SPACING;

/// Unthemed-fallback `onSurface` (M3 baseline light).
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback `onSurfaceVariant`.
const FALLBACK_ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `primary`.
const FALLBACK_PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `onPrimary`.
const FALLBACK_ON_PRIMARY: Color = Color::WHITE;

/// The colors one calendar paint pass resolves once, up front — the theme
/// borrows `PaintCtx`, and the motion arms need it mutably (the ordering
/// [`crate::search::bar`] documents).
#[derive(Clone, Copy)]
struct Palette {
    on_surface: Color,
    on_surface_variant: Color,
    primary: Color,
    on_primary: Color,
}

impl Palette {
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(t) => {
                let s = t.scheme();
                Self {
                    on_surface: s.on_surface,
                    on_surface_variant: s.on_surface_variant,
                    primary: s.primary,
                    on_primary: s.on_primary,
                }
            }
            None => Self {
                on_surface: FALLBACK_ON_SURFACE,
                on_surface_variant: FALLBACK_ON_SURFACE_VARIANT,
                primary: FALLBACK_PRIMARY,
                on_primary: FALLBACK_ON_PRIMARY,
            },
        }
    }
}

/// `color` with its alpha replaced by `alpha` (the M3 "content color at N%"
/// shape, not a multiply).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// What a pointer is over. Derived from the geometry `layout` recorded — this
/// widget has no pods to hit-test through (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    /// The previous-month chevron.
    PrevMonth,
    /// The next-month chevron.
    NextMonth,
    /// The month/year label, which toggles the calendar sub-view.
    ModeToggle,
    /// A day cell holding a real, selectable date.
    Day(MaterialDate),
    /// A year cell.
    Year(i32),
}

/// A caller-supplied "is this day selectable?" predicate — the reference's
/// `M3ESelectableDayPredicate`.
pub type SelectableDay = Rc<dyn Fn(MaterialDate) -> bool>;

/// A declarative M3E calendar. See the [module docs](self).
pub struct CalendarDatePicker<State: 'static> {
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    today: Option<MaterialDate>,
    range: Option<DateRange>,
    strings: DatePickerStrings,
    selectable: Option<SelectableDay>,
    on_change: TypedArgCallback<State, DatePickerState>,
}

/// Create a controlled calendar showing `state`, bounded by
/// `first_date..=last_date`, reporting every requested state change through
/// `on_change`.
///
/// A reversed bound pair is normalized (the earlier date wins as `first_date`)
/// rather than asserted, unlike the reference's
/// `assert(!lastDate.isBefore(firstDate))` — a widget has no useful panic here.
///
/// ```ignore
/// calendar_date_picker(
///     app.picker.clone(),
///     MaterialDate::new(2020, 1, 1),
///     MaterialDate::new(2030, 12, 31),
///     |app: &mut App, next| app.picker = next,
/// )
/// .today(MaterialDate::new(2026, 8, 20))
/// ```
pub fn calendar_date_picker<State: 'static, F>(
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    on_change: F,
) -> CalendarDatePicker<State>
where
    F: Fn(&mut State, DatePickerState) + 'static,
{
    let (first_date, last_date) = if last_date < first_date {
        (last_date, first_date)
    } else {
        (first_date, last_date)
    };
    CalendarDatePicker {
        state,
        first_date,
        last_date,
        today: None,
        range: None,
        strings: DatePickerStrings::ENGLISH,
        selectable: None,
        on_change: Rc::new(on_change),
    }
}

impl<State: 'static> CalendarDatePicker<State> {
    /// Mark `today` with the today ring (the reference's `currentDate`).
    ///
    /// **Required for a today ring to appear at all.** This crate reads no
    /// clock: `frust-core`/widget code never calls `Instant::now()`
    /// (`docs/REVIEW_FOCUS.md`'s theme-resolution hot spot), and a design-system
    /// plugin has no shell-supplied wall-clock seam either — the reference's
    /// `DateTime.now()` fallback has no equivalent here, so an app that wants
    /// the ring passes the date it already knows.
    pub fn today(mut self, today: MaterialDate) -> Self {
        self.today = Some(today);
        self
    }

    /// Paint `range`'s in-range fill and start/end caps across the grid — the
    /// reference's `M3EDayCell` `inRange`/`rangeStart`/`rangeEnd` visuals. See
    /// [`DateRange`] for what this port does and does not carry of the range
    /// picker.
    pub fn range(mut self, range: DateRange) -> Self {
        self.range = Some(range);
        self
    }

    /// Replace the localized strings and the first-day-of-week index (default
    /// [`DatePickerStrings::ENGLISH`]).
    pub fn strings(mut self, strings: DatePickerStrings) -> Self {
        self.strings = strings;
        self
    }

    /// Refuse individual days inside the bounds — the reference's
    /// `selectableDayPredicate`. A refused day paints disabled and reports
    /// nothing on tap.
    pub fn selectable<F: Fn(MaterialDate) -> bool + 'static>(mut self, predicate: F) -> Self {
        self.selectable = Some(Rc::new(predicate));
        self
    }
}

impl<State: 'static> View<State> for CalendarDatePicker<State> {
    type Element = CalendarDatePickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CalendarDatePickerWidget {
        CalendarDatePickerWidget {
            state: self.state.clone(),
            first_date: self.first_date,
            last_date: self.last_date,
            today: self.today,
            range: self.range,
            strings: self.strings,
            selectable: self.selectable.clone(),
            on_change: erase_callback_arg(&self.on_change),
            runs: Runs::new(),
            glyphs: Glyphs::new(),
            geometry: Geometry::default(),
            year_scroll: 0.0,
            year_scroll_seeded: false,
            slide: AnimationController::new(MONTH_SCROLL_DURATION)
                .with_curve(MaterialMotion::STANDARD),
            slide_from: None,
            slide_dir: 0,
            hovered: None,
            pressed: None,
            captured: false,
            drag_from: None,
            dragging: false,
            layer: StateLayer::new(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CalendarDatePickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_change = erase_callback_arg(&self.on_change);
        element.selectable = self.selectable.clone();

        let mut flags = ChangeFlags::NONE;
        if prev.state.displayed_month != self.state.displayed_month {
            // The slide direction is the sign of the month delta: a jump of
            // several months still slides once (see the module docs).
            let delta = prev
                .state
                .displayed_month
                .month_delta(self.state.displayed_month);
            element.start_slide(prev.state.displayed_month, delta.signum());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.state.mode != self.state.mode {
            // Re-seed the year grid's scroll offset the next time it lays out
            // (`_M3EYearPickerState._initialOffset`).
            element.year_scroll_seeded = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.state != self.state {
            element.state = self.state.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.first_date != self.first_date
            || prev.last_date != self.last_date
            || prev.today != self.today
            || prev.range != self.range
        {
            element.first_date = self.first_date;
            element.last_date = self.last_date;
            element.today = self.today;
            element.range = self.range;
            flags |= ChangeFlags::PAINT;
        }
        if prev.strings != self.strings {
            element.strings = self.strings;
            element.runs.invalidate_all();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, _element: &mut CalendarDatePickerWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The retained text runs a calendar re-brushes each paint. See the
/// [module docs](self)' no-child-pods note.
struct Runs {
    /// `"1"`..`"31"`, indexed by `day - 1`; shared by both months during a slide.
    days: Vec<Option<TextLayout>>,
    /// The seven weekday initials, in *column* order (already rotated by
    /// `first_day_of_week`).
    weekdays: Vec<TextLayout>,
    /// The sub-header label, with the text it was shaped from.
    header: Option<(String, TextLayout)>,
    /// Year labels, keyed by year — populated for whatever the year grid
    /// scrolls past.
    years: HashMap<i32, TextLayout>,
}

impl Runs {
    fn new() -> Self {
        Self {
            days: (0..31).map(|_| None).collect(),
            weekdays: Vec::new(),
            header: None,
            years: HashMap::new(),
        }
    }

    /// Drop every shaped run — what a strings change invalidates.
    fn invalidate_all(&mut self) {
        self.weekdays.clear();
        self.header = None;
        self.years.clear();
    }

    fn day(&mut self, ctx: &mut LayoutCtx, day: u32) -> &TextLayout {
        let index = (day.clamp(1, 31) - 1) as usize;
        if self.days[index].is_none() {
            let style = TextStyle::new(DAY_TEXT_SIZE, SHAPING_INK);
            let text = (index + 1).to_string();
            let laid = ctx
                .text_context::<TextContext>()
                .layout(&text, &style, None);
            self.days[index] = Some(laid);
        }
        self.days[index].as_ref().expect("just shaped")
    }

    fn weekdays(&mut self, ctx: &mut LayoutCtx, strings: &DatePickerStrings) {
        if !self.weekdays.is_empty() {
            return;
        }
        let style = TextStyle::new(WEEKDAY_TEXT_SIZE, SHAPING_INK);
        for column in 0..DAYS_PER_WEEK {
            let laid = ctx.text_context::<TextContext>().layout(
                strings.weekday_initial(column),
                &style,
                None,
            );
            self.weekdays.push(laid);
        }
    }

    fn header(&mut self, ctx: &mut LayoutCtx, text: &str) -> &TextLayout {
        let stale = self
            .header
            .as_ref()
            .is_none_or(|(cached, _)| cached != text);
        if stale {
            let style = TextStyle {
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(SUB_HEADER_TEXT_SIZE, SHAPING_INK)
            };
            let laid = ctx.text_context::<TextContext>().layout(text, &style, None);
            self.header = Some((text.to_string(), laid));
        }
        &self.header.as_ref().expect("just shaped").1
    }

    fn year(&mut self, ctx: &mut LayoutCtx, year: i32) -> &TextLayout {
        self.years.entry(year).or_insert_with(|| {
            let style = TextStyle {
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(YEAR_TEXT_SIZE, SHAPING_INK)
            };
            ctx.text_context::<TextContext>()
                .layout(&year.to_string(), &style, None)
        })
    }
}

/// The four sub-header icon paths, parsed once at build. A [`Glyph`] parses its
/// `BezPath` on construction, so building one per paint would re-parse four
/// paths every frame.
struct Glyphs {
    arrow_down: Glyph,
    arrow_up: Glyph,
    chevron_left: Glyph,
    chevron_right: Glyph,
}

impl Glyphs {
    fn new() -> Self {
        Self {
            arrow_down: Glyph::new(crate::icons::ARROW_DROP_DOWN, ARROW_ICON_SIZE),
            arrow_up: Glyph::new(crate::icons::ARROW_DROP_UP, ARROW_ICON_SIZE),
            chevron_left: Glyph::new(crate::icons::CHEVRON_LEFT, ARROW_ICON_SIZE),
            chevron_right: Glyph::new(crate::icons::CHEVRON_RIGHT, ARROW_ICON_SIZE),
        }
    }
}

/// The hit/paint geometry one `layout` pass resolves. Recorded on the widget
/// because this widget hit-tests and reports semantics from it rather than from
/// child pods.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Geometry {
    size: Size,
    /// The month/year label's own tappable box (label + drop arrow).
    toggle: Rect,
    /// The prev/next chevrons — empty rects in the year sub-view, which has
    /// none.
    prev: Rect,
    next: Rect,
    /// The day grid's top-left and one cell's size.
    grid_origin: Point,
    cell: Size,
    /// The year grid's clipped viewport, and one year cell's size.
    year_viewport: Rect,
    year_cell: Size,
}

/// The retained widget for a [`CalendarDatePicker`].
pub struct CalendarDatePickerWidget {
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    today: Option<MaterialDate>,
    range: Option<DateRange>,
    strings: DatePickerStrings,
    selectable: Option<SelectableDay>,
    on_change: ErasedArgCallback<DatePickerState>,
    runs: Runs,
    glyphs: Glyphs,
    geometry: Geometry,
    /// The year grid's scroll offset, in logical px from the grid's own top.
    year_scroll: f64,
    year_scroll_seeded: bool,
    /// The month-paging slide. See the [module docs](self).
    slide: AnimationController,
    /// The month sliding *out*, while one is.
    slide_from: Option<MaterialDate>,
    /// `-1` sliding to an earlier month, `+1` to a later one, `0` at rest.
    slide_dir: i32,
    hovered: Option<Hit>,
    pressed: Option<Hit>,
    captured: bool,
    /// Where a year-grid drag started, before it passes the slop.
    drag_from: Option<Point>,
    dragging: bool,
    layer: StateLayer,
}

impl CalendarDatePickerWidget {
    /// Whether `date` is inside the bounds *and* accepted by the caller's
    /// predicate — the reference's `M3EDatePickerUtils.isSelectable`.
    fn is_selectable(&self, date: MaterialDate) -> bool {
        date.is_within(self.first_date, self.last_date)
            && self.selectable.as_ref().is_none_or(|p| p(date))
    }

    /// The grid for the month currently displayed.
    fn grid(&self) -> MonthGrid {
        MonthGrid::new(self.state.displayed_month, self.strings.first_day_of_week)
    }

    /// How many years the bounds span.
    fn year_count(&self) -> usize {
        grid::year_span(self.first_date, self.last_date)
    }

    /// Start a page slide away from `from`, in direction `dir`.
    fn start_slide(&mut self, from: MaterialDate, dir: i32) {
        if dir == 0 {
            return;
        }
        self.slide_from = Some(from);
        self.slide_dir = dir;
        self.slide =
            AnimationController::new(MONTH_SCROLL_DURATION).with_curve(MaterialMotion::STANDARD);
        self.slide.forward();
    }

    /// Pin the slide at its end with no motion (`reduce_motion`).
    fn settle_slide(&mut self) {
        self.slide_from = None;
        self.slide_dir = 0;
    }

    /// The largest scroll offset the year grid can take.
    fn max_year_scroll(&self) -> f64 {
        (grid::year_grid_natural_height(self.year_count()) - self.geometry.year_viewport.height())
            .max(0.0)
    }

    /// The offset that puts the selected year's row at the viewport's top —
    /// `_M3EYearPickerState._initialOffset`.
    fn initial_year_scroll(&self) -> f64 {
        let selected = self
            .state
            .selected
            .unwrap_or(self.state.displayed_month)
            .year();
        let index = (selected - self.first_date.year()).max(0) as usize;
        let row = index / YEAR_COLUMN_COUNT;
        (row as f64 * (YEAR_ROW_HEIGHT + YEAR_ROW_SPACING)).clamp(0.0, self.max_year_scroll())
    }

    /// Clamp the scroll offset back into range (after a resize, a bounds
    /// change, or a fling past the end).
    fn clamp_year_scroll(&mut self) {
        self.year_scroll = self.year_scroll.clamp(0.0, self.max_year_scroll());
    }

    /// The year in cell `index` of the grid.
    fn year_at(&self, index: usize) -> Option<i32> {
        if index >= self.year_count() {
            return None;
        }
        Some(self.first_date.year() + index as i32)
    }

    /// The local-space rect of year cell `index`, already scrolled.
    fn year_cell_rect(&self, index: usize) -> Rect {
        let row = (index / YEAR_COLUMN_COUNT) as f64;
        let column = (index % YEAR_COLUMN_COUNT) as f64;
        let v = self.geometry.year_viewport;
        let x =
            v.x0 + YEAR_GRID_PADDING + column * (self.geometry.year_cell.width + YEAR_ROW_SPACING);
        let y = v.y0 + YEAR_GRID_PADDING + row * (YEAR_ROW_HEIGHT + YEAR_ROW_SPACING)
            - self.year_scroll;
        Rect::from_origin_size(Point::new(x, y), self.geometry.year_cell)
    }

    /// The local-space rect of day cell `index` in the displayed month's grid.
    fn day_cell_rect(&self, index: usize) -> Rect {
        let row = (index / DAYS_PER_WEEK) as f64;
        let column = (index % DAYS_PER_WEEK) as f64;
        Rect::from_origin_size(
            Point::new(
                self.geometry.grid_origin.x + column * self.geometry.cell.width,
                self.geometry.grid_origin.y + row * self.geometry.cell.height,
            ),
            self.geometry.cell,
        )
    }

    /// What lies under local-space point `p`, if anything actionable does.
    fn hit(&self, p: Point) -> Option<Hit> {
        if self.geometry.toggle.contains(p) {
            return Some(Hit::ModeToggle);
        }
        match self.state.mode {
            DatePickerMode::Day => {
                if self.geometry.prev.contains(p) {
                    return Some(Hit::PrevMonth);
                }
                if self.geometry.next.contains(p) {
                    return Some(Hit::NextMonth);
                }
                let grid = self.grid();
                for index in 0..grid.cell_count() {
                    if !self.day_cell_rect(index).contains(p) {
                        continue;
                    }
                    let date = grid.date_at(index)?;
                    return self.is_selectable(date).then_some(Hit::Day(date));
                }
                None
            }
            DatePickerMode::Year => {
                if !self.geometry.year_viewport.contains(p) {
                    return None;
                }
                for index in 0..self.year_count() {
                    if self.year_cell_rect(index).contains(p) {
                        let year = self.year_at(index)?;
                        let candidate =
                            self.state
                                .year_candidate(year, self.first_date, self.last_date);
                        return self.is_selectable(candidate).then_some(Hit::Year(year));
                    }
                }
                None
            }
        }
    }

    /// The state a completed press on `hit` requests.
    fn next_state(&self, hit: Hit) -> DatePickerState {
        match hit {
            Hit::PrevMonth => self
                .state
                .stepped_month(-1, self.first_date, self.last_date),
            Hit::NextMonth => self.state.stepped_month(1, self.first_date, self.last_date),
            Hit::ModeToggle => self.state.toggled_mode(),
            Hit::Day(date) => self.state.with_selected(date),
            Hit::Year(year) => self.state.with_year(year, self.first_date, self.last_date),
        }
    }
}

impl<State: 'static> CalendarDatePicker<State> {
    /// This calendar's intrinsic height for `width`-independent content — the
    /// reference's `_inlinePickerBodyHeight` plus its own sub-header band.
    ///
    /// Public so a host that must reserve the calendar's box before laying it
    /// out (the dialog does, to size its animated body) can ask without
    /// painting.
    pub fn intrinsic_height(&self) -> f64 {
        intrinsic_height(
            &self.state,
            self.first_date,
            self.last_date,
            self.strings.first_day_of_week,
        )
    }
}

/// The calendar's intrinsic height for `state` — see
/// [`CalendarDatePicker::intrinsic_height`].
pub(crate) fn intrinsic_height(
    state: &DatePickerState,
    first: MaterialDate,
    last: MaterialDate,
    first_day_of_week: u32,
) -> f64 {
    match state.mode {
        DatePickerMode::Day => {
            grid::calendar_day_view_height(state.displayed_month, first_day_of_week)
        }
        DatePickerMode::Year => grid::calendar_year_view_height(first, last, true),
    }
}

impl Widget for CalendarDatePickerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max_w = bc.max().width;
        let width = if max_w.is_finite() {
            max_w.max(bc.min().width)
        } else {
            CALENDAR_WIDTH
        }
        .max(1.0);

        // Shape everything this pass may paint. The day digits are shared by
        // both months during a slide, so one pass over 1..=31 covers both.
        self.runs.weekdays(ctx, &self.strings);
        let header_text = match self.state.mode {
            DatePickerMode::Day => self.strings.month_year(self.state.displayed_month),
            DatePickerMode::Year => self.strings.year(self.state.displayed_month.year()),
        };
        let header_size = self.runs.header(ctx, &header_text).size();

        let toggle_w = (header_size.width + ARROW_ICON_SIZE).min(width - SUB_HEADER_START_INSET);
        let toggle = Rect::from_origin_size(
            Point::new(SUB_HEADER_START_INSET, 0.0),
            Size::new(toggle_w.max(0.0), SUB_HEADER_HEIGHT),
        );

        // Two 40dp affordances right-aligned inside the 108dp nav band
        // (`monthNavButtonsWidth`, `MainAxisAlignment.end`).
        let button = ARROW_ICON_SIZE + 2.0 * ARROW_PADDING;
        let (prev, next) = if matches!(self.state.mode, DatePickerMode::Day) {
            let next_x = width - button;
            let prev_x = next_x - button;
            (
                Rect::from_origin_size(
                    Point::new(prev_x, 0.0),
                    Size::new(button, SUB_HEADER_HEIGHT),
                ),
                Rect::from_origin_size(
                    Point::new(next_x, 0.0),
                    Size::new(button, SUB_HEADER_HEIGHT),
                ),
            )
        } else {
            (Rect::ZERO, Rect::ZERO)
        };

        let height = match self.state.mode {
            DatePickerMode::Day => {
                for day in 1..=31u32 {
                    self.runs.day(ctx, day);
                }
                grid::calendar_day_view_height(
                    self.state.displayed_month,
                    self.strings.first_day_of_week,
                )
            }
            DatePickerMode::Year => {
                // Every year in the span, not just the visible window: a
                // scroll is a *paint*-only change (`EventCtx` carries no
                // relayout seam — only `request_redraw`), so a year first
                // scrolled into view would otherwise have no shaped label to
                // paint. The runs are cached, so this is one pass per year for
                // the widget's whole life.
                for index in 0..self.year_count() {
                    if let Some(year) = self.year_at(index) {
                        self.runs.year(ctx, year);
                    }
                }
                SUB_HEADER_HEIGHT + grid::year_grid_height(self.year_count())
            }
        };

        let cell_w = width / DAYS_PER_WEEK as f64;
        let year_viewport = Rect::from_origin_size(
            Point::new(0.0, SUB_HEADER_HEIGHT),
            Size::new(width, grid::year_grid_height(self.year_count())),
        );
        let year_cell_w = ((width
            - 2.0 * YEAR_GRID_PADDING
            - (YEAR_COLUMN_COUNT as f64 - 1.0) * YEAR_ROW_SPACING)
            / YEAR_COLUMN_COUNT as f64)
            .max(0.0);

        self.geometry = Geometry {
            size: Size::new(width, height),
            toggle,
            prev,
            next,
            grid_origin: Point::new(
                0.0,
                SUB_HEADER_HEIGHT + WEEKDAY_ROW_HEIGHT + DAY_GRID_TOP_PADDING,
            ),
            cell: Size::new(cell_w, DAY_ROW_HEIGHT),
            year_viewport,
            year_cell: Size::new(year_cell_w, YEAR_ROW_HEIGHT),
        };

        if matches!(self.state.mode, DatePickerMode::Year) && !self.year_scroll_seeded {
            self.year_scroll = self.initial_year_scroll();
            self.year_scroll_seeded = true;
        }
        self.clamp_year_scroll();

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce, palette) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                Palette::resolve(theme),
            )
        };
        if reduce {
            self.settle_slide();
        } else if self.slide.advance(ctx.frame_time()) {
            ctx.request_frame();
        } else if self.slide_dir != 0 && !self.slide.is_animating() {
            self.settle_slide();
        }

        // The pod's hover path is authoritative (`docs/CODE_STANDARDS.md`'s
        // Interaction Semantics): a pointer that left without a `Move` reaching
        // us still clears here.
        if !ctx.is_hovered() && self.hovered.is_some() {
            self.hovered = None;
        }
        self.layer.set_hovered(self.hovered.is_some());

        let origin = ctx.origin();
        self.paint_sub_header(ctx, scene, origin, palette);
        match self.state.mode {
            DatePickerMode::Day => self.paint_day_view(scene, origin, palette),
            DatePickerMode::Year => self.paint_year_view(scene, origin, palette),
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Scroll { position, delta } => {
                if !matches!(self.state.mode, DatePickerMode::Year)
                    || !self.geometry.year_viewport.contains(*position)
                    || self.max_year_scroll() <= 0.0
                {
                    return EventResult::Ignored;
                }
                let dy = match delta {
                    ScrollDelta::Lines(_, y) => y * YEAR_WHEEL_LINE,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                let before = self.year_scroll;
                self.year_scroll -= dy;
                self.clamp_year_scroll();
                if self.year_scroll != before {
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(hit) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.pressed = Some(hit);
                    self.captured = true;
                    self.dragging = false;
                    self.drag_from =
                        matches!(self.state.mode, DatePickerMode::Year).then_some(p.position);
                    self.layer.set_pressed(true);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if !self.captured {
                        let over = self.hit(p.position);
                        if over.is_some() {
                            ctx.claim_hover();
                        }
                        if self.hovered != over {
                            self.hovered = over;
                            self.layer.set_hovered(over.is_some());
                            ctx.request_redraw();
                        }
                        return EventResult::Ignored;
                    }
                    // A year-grid press that travels past the slop turns into a
                    // scroll and gives up its tap.
                    if let Some(from) = self.drag_from {
                        let travel = (p.position - from).hypot();
                        if self.dragging || travel > TOUCH_SLOP {
                            if !self.dragging {
                                self.dragging = true;
                                self.pressed = None;
                                self.layer.set_pressed(false);
                            }
                            let before = self.year_scroll;
                            self.year_scroll -= p.position.y - from.y;
                            self.clamp_year_scroll();
                            self.drag_from = Some(p.position);
                            if self.year_scroll != before {
                                ctx.request_redraw();
                            }
                            return EventResult::Handled;
                        }
                    }
                    let still_on = self
                        .pressed
                        .filter(|hit| self.hit(p.position) == Some(*hit));
                    if self.pressed.is_some() && still_on.is_none() {
                        self.layer.set_pressed(false);
                        ctx.request_redraw();
                    }
                    self.pressed = still_on;
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    let hit = self
                        .pressed
                        .filter(|hit| !self.dragging && self.hit(p.position) == Some(*hit));
                    if let Some(hit) = hit {
                        // The reference fires `M3EHaptics.selection()` on a day,
                        // a year, and a sub-view toggle alike; `SliderTick` is
                        // this crate's discrete-selection signal.
                        MaterialHaptics::fire(HapticSignal::SliderTick);
                        let next = self.next_state(hit);
                        (self.on_change)(ctx, next);
                    }
                    self.pressed = None;
                    self.captured = false;
                    self.dragging = false;
                    self.drag_from = None;
                    self.layer.set_pressed(false);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    // A `Cancel` arm only clears flags — never `state_mut`
                    // (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
                    self.pressed = None;
                    self.captured = false;
                    self.dragging = false;
                    self.drag_from = None;
                    self.layer.set_pressed(false);
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let origin = ctx.origin();
        let label = match self.state.mode {
            DatePickerMode::Day => self.strings.month_year(self.state.displayed_month),
            DatePickerMode::Year => self.strings.select_year.to_string(),
        };
        let mode = self.state.mode;
        let grid = self.grid();
        let cells: Vec<(Rect, String, bool)> = match mode {
            DatePickerMode::Day => (0..grid.cell_count())
                .filter_map(|index| {
                    let date = grid.date_at(index)?;
                    Some((
                        self.day_cell_rect(index),
                        date.day().to_string(),
                        self.is_selectable(date),
                    ))
                })
                .collect(),
            DatePickerMode::Year => (0..self.year_count())
                .filter_map(|index| {
                    let rect = self.year_cell_rect(index);
                    if rect.y1 < self.geometry.year_viewport.y0
                        || rect.y0 > self.geometry.year_viewport.y1
                    {
                        return None;
                    }
                    let year = self.year_at(index)?;
                    let candidate =
                        self.state
                            .year_candidate(year, self.first_date, self.last_date);
                    Some((rect, self.strings.year(year), self.is_selectable(candidate)))
                })
                .collect(),
        };

        ctx.push_container(
            Role::Group,
            |node| node.set_label(label),
            |ctx| {
                for (rect, text, enabled) in cells {
                    ctx.push_node(Role::Button, |node| {
                        node.set_bounds(frust::accesskit::Rect {
                            x0: origin.x + rect.x0,
                            y0: origin.y + rect.y0,
                            x1: origin.x + rect.x1,
                            y1: origin.y + rect.y1,
                        });
                        node.set_label(text);
                        if enabled {
                            node.add_action(Action::Click);
                        } else {
                            node.set_disabled();
                        }
                    });
                }
            },
        );
    }
}

// ---- paint helpers ---------------------------------------------------------

impl CalendarDatePickerWidget {
    fn paint_sub_header(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        origin: Point,
        palette: Palette,
    ) {
        let toggle = self.geometry.toggle;
        let label_size = self
            .runs
            .header
            .as_ref()
            .map_or(Size::ZERO, |(_, l)| l.size());
        let label_origin = Point::new(
            origin.x + toggle.x0,
            origin.y + (SUB_HEADER_HEIGHT - label_size.height) / 2.0,
        );
        if let Some((_, layout)) = &self.runs.header {
            for mut run in layout.to_scene_runs(label_origin) {
                run.brush = Brush::Solid(palette.on_surface);
                scene.draw_glyph_run(run);
            }
        }

        // The drop arrow points down in the day sub-view and up in the year one
        // (`M3EDatePickerModeToggle`).
        let arrow = match self.state.mode {
            DatePickerMode::Day => &self.glyphs.arrow_down,
            DatePickerMode::Year => &self.glyphs.arrow_up,
        };
        arrow.paint(
            Point::new(
                label_origin.x + label_size.width,
                origin.y + (SUB_HEADER_HEIGHT - ARROW_ICON_SIZE) / 2.0,
            ),
            palette.on_surface,
            scene,
        );

        // The press/hover overlay under whichever affordance is engaged.
        let engaged = self.pressed.or(self.hovered);
        if let Some(rect) = engaged.and_then(|hit| match hit {
            Hit::ModeToggle => Some(toggle),
            Hit::PrevMonth => Some(self.geometry.prev),
            Hit::NextMonth => Some(self.geometry.next),
            _ => None,
        }) {
            let abs = Rect::from_origin_size(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                rect.size(),
            );
            self.layer
                .paint(ctx, scene, abs, abs.height() / 2.0, palette.on_surface);
        }

        if matches!(self.state.mode, DatePickerMode::Year) {
            return;
        }
        for (rect, icon, delta) in [
            (self.geometry.prev, &self.glyphs.chevron_left, -1),
            (self.geometry.next, &self.glyphs.chevron_right, 1),
        ] {
            let enabled = self
                .state
                .can_step_month(delta, self.first_date, self.last_date);
            let ink = if enabled {
                palette.on_surface_variant
            } else {
                with_alpha(palette.on_surface, DISABLED_DAY_OPACITY)
            };
            icon.paint(
                Point::new(
                    origin.x + rect.x0 + ARROW_PADDING,
                    origin.y + (SUB_HEADER_HEIGHT - ARROW_ICON_SIZE) / 2.0,
                ),
                ink,
                scene,
            );
        }
    }

    fn paint_day_view(&mut self, scene: &mut dyn PaintScene, origin: Point, palette: Palette) {
        // Weekday initials, one per column, centered.
        let band_y = origin.y + SUB_HEADER_HEIGHT;
        for column in 0..DAYS_PER_WEEK {
            let Some(layout) = self.runs.weekdays.get(column) else {
                break;
            };
            let size = layout.size();
            let x = origin.x + (column as f64 + 0.5) * self.geometry.cell.width - size.width / 2.0;
            let y = band_y + (WEEKDAY_ROW_HEIGHT - size.height) / 2.0;
            for mut run in layout.to_scene_runs(Point::new(x, y)) {
                run.brush = Brush::Solid(palette.on_surface_variant);
                scene.draw_glyph_run(run);
            }
        }

        // The grid area clips the sliding pages.
        let grid_top = self.geometry.grid_origin.y;
        let clip_origin = Point::new(origin.x, origin.y + grid_top);
        let clip_size = Size::new(
            self.geometry.size.width,
            (self.geometry.size.height - grid_top).max(0.0),
        );
        scene.push_clip(clip_origin, clip_size);

        let width = self.geometry.size.width;
        let t = self.slide.value_clamped();
        let (incoming_dx, outgoing) = match (self.slide_from, self.slide_dir) {
            (Some(from), dir) if dir != 0 => (
                dir as f64 * width * (1.0 - t),
                Some((from, -(dir as f64) * width * t)),
            ),
            _ => (0.0, None),
        };
        if let Some((month, dx)) = outgoing {
            self.paint_month(scene, origin, palette, month, dx, false);
        }
        self.paint_month(
            scene,
            origin,
            palette,
            self.state.displayed_month,
            incoming_dx,
            true,
        );
        scene.pop_clip();
    }

    /// One month's day cells, shifted `dx` along the page axis. `live` marks the
    /// month the user can actually press (an outgoing page paints no press
    /// overlay).
    fn paint_month(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        palette: Palette,
        month: MaterialDate,
        dx: f64,
        live: bool,
    ) {
        let grid = MonthGrid::new(month, self.strings.first_day_of_week);
        for index in 0..grid.cell_count() {
            let Some(date) = grid.date_at(index) else {
                continue;
            };
            let rect = self.day_cell_rect(index);
            let cell = Rect::from_origin_size(
                Point::new(origin.x + rect.x0 + dx, origin.y + rect.y0),
                rect.size(),
            );
            let center = cell.center();
            let enabled = self.is_selectable(date);
            let selected = self.state.selected == Some(date);
            let range_start = self.range.is_some_and(|r| r.start == date);
            let range_end = self.range.and_then(|r| r.end) == Some(date);
            let in_range = self
                .range
                .is_some_and(|r| r.is_complete() && r.contains(date));
            let capped = range_start || range_end;

            // The range band: a full-width fill between the caps, and a
            // half-width lead-in/out under each cap (`M3EDayCell`'s
            // `Positioned` pair).
            if in_range {
                let fill = with_alpha(palette.primary, RANGE_HIGHLIGHT_ALPHA);
                let band_h = DAY_SIZE;
                let band_y = center.y - band_h / 2.0;
                let (x0, x1) = if range_start && range_end {
                    (center.x, center.x)
                } else if range_start {
                    (center.x, cell.x1)
                } else if range_end {
                    (cell.x0, center.x)
                } else {
                    (cell.x0, cell.x1)
                };
                if x1 > x0 {
                    scene.fill_rect(Point::new(x0, band_y), Size::new(x1 - x0, band_h), fill);
                }
            }

            if selected || capped {
                scene.fill_path(
                    Point::ZERO,
                    &Circle::new(center, DAY_SIZE / 2.0).to_path(PATH_TOLERANCE),
                    &Brush::Solid(palette.primary),
                );
            } else if self.today == Some(date) {
                scene.stroke_path(
                    Point::ZERO,
                    &Circle::new(center, DAY_SIZE / 2.0 - TODAY_RING_WIDTH / 2.0)
                        .to_path(PATH_TOLERANCE),
                    TODAY_RING_WIDTH,
                    &Brush::Solid(palette.primary),
                );
            }

            let ink = if !enabled {
                with_alpha(palette.on_surface, DISABLED_DAY_OPACITY)
            } else if selected || capped {
                palette.on_primary
            } else {
                palette.on_surface
            };

            // The state layer sits *over* whatever the cell already painted,
            // in the cell's own content color — the M3 "content color at N%"
            // rule [`crate::state_layer`] implements for a pod-backed control.
            let engaged = live && self.pressed.or(self.hovered) == Some(Hit::Day(date));
            if engaged && self.layer.opacity() > 0.0 {
                scene.fill_path(
                    Point::ZERO,
                    &Circle::new(center, DAY_SIZE / 2.0).to_path(PATH_TOLERANCE),
                    &Brush::Solid(with_alpha(ink, self.layer.opacity())),
                );
            }
            if let Some(layout) = self.runs.days[(date.day() - 1) as usize].as_ref() {
                let size = layout.size();
                let at = Point::new(center.x - size.width / 2.0, center.y - size.height / 2.0);
                for mut run in layout.to_scene_runs(at) {
                    run.brush = Brush::Solid(ink);
                    scene.draw_glyph_run(run);
                }
            }
        }
    }

    fn paint_year_view(&mut self, scene: &mut dyn PaintScene, origin: Point, palette: Palette) {
        let viewport = self.geometry.year_viewport;
        scene.push_clip(
            Point::new(origin.x + viewport.x0, origin.y + viewport.y0),
            viewport.size(),
        );
        let selected_year = self
            .state
            .selected
            .unwrap_or(self.state.displayed_month)
            .year();
        for index in 0..self.year_count() {
            let rect = self.year_cell_rect(index);
            if rect.y1 < viewport.y0 || rect.y0 > viewport.y1 {
                continue;
            }
            let Some(year) = self.year_at(index) else {
                continue;
            };
            let candidate = self
                .state
                .year_candidate(year, self.first_date, self.last_date);
            let enabled = self.is_selectable(candidate);
            let selected = year == selected_year;
            let cell = Rect::from_origin_size(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                rect.size(),
            );
            let center = cell.center();
            let radius = cell.height() / 2.0;
            if selected {
                scene.fill_rounded_rect(cell.origin(), cell.size(), radius, palette.primary);
            }
            let ink = if !enabled {
                with_alpha(palette.on_surface, DISABLED_DAY_OPACITY)
            } else if selected {
                palette.on_primary
            } else {
                palette.on_surface
            };
            // Same over-the-fill state layer the day cells take.
            if self.pressed.or(self.hovered) == Some(Hit::Year(year)) && self.layer.opacity() > 0.0
            {
                scene.fill_rounded_rect(
                    cell.origin(),
                    cell.size(),
                    radius,
                    with_alpha(ink, self.layer.opacity()),
                );
            }
            if let Some(layout) = self.runs.years.get(&year) {
                let size = layout.size();
                let at = Point::new(center.x - size.width / 2.0, center.y - size.height / 2.0);
                for mut run in layout.to_scene_runs(at) {
                    run.brush = Brush::Solid(ink);
                    scene.draw_glyph_run(run);
                }
            }
        }
        scene.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::super::MONTH_NAV_BUTTONS_WIDTH;
    use super::*;
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc as StdRc;

    fn d(year: i32, month: u32, day: u32) -> MaterialDate {
        MaterialDate::new(year, month, day)
    }

    const FIRST: fn() -> MaterialDate = || MaterialDate::new(2020, 1, 1);
    const LAST: fn() -> MaterialDate = || MaterialDate::new(2030, 12, 31);

    /// The app state a test calendar reports into.
    #[derive(Default)]
    struct App {
        reported: Vec<DatePickerState>,
    }

    fn view(state: DatePickerState) -> CalendarDatePicker<App> {
        calendar_date_picker(state, FIRST(), LAST(), |app: &mut App, next| {
            app.reported.push(next)
        })
    }

    fn build(view: &CalendarDatePicker<App>) -> CalendarDatePickerWidget {
        let mut counter = 0u64;
        View::<App>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn laid_out(view: &CalendarDatePicker<App>, width: f64) -> (CalendarDatePickerWidget, Size) {
        let mut widget = build(view);
        let size = layout_at(&mut widget, width);
        (widget, size)
    }

    fn layout_at(widget: &mut CalendarDatePickerWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(
            &mut lctx,
            &BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY)),
        )
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        fills: Vec<(Point, Color)>,
        strokes: Vec<(Point, f64, Color)>,
        runs: Vec<(Point, Color, usize)>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn push_clip(&mut self, _o: Point, _s: Size) {
            self.clips += 1;
        }
        fn pop_clip(&mut self) {
            self.clips -= 1;
        }
        fn fill_path(&mut self, o: Point, path: &kurbo::BezPath, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            let at = path
                .elements()
                .first()
                .and_then(|e| match e {
                    kurbo::PathEl::MoveTo(p) => Some(*p),
                    _ => None,
                })
                .unwrap_or(o);
            self.fills.push((at, color));
        }
        fn stroke_path(&mut self, o: Point, _path: &kurbo::BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((o, width, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            let color = match &run.brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.runs
                .push((Point::new(t.x, t.y), color, run.glyphs.len()));
        }
    }

    /// How many *day-circle* fills a pass emitted — the sub-header's chevrons
    /// and drop arrow fill paths too, so a bare count would include them; only
    /// a day/year circle is inked `primary`.
    fn primary_fills(rec: &Recorder) -> usize {
        rec.fills
            .iter()
            .filter(|(_, color)| *color == FALLBACK_PRIMARY)
            .count()
    }

    fn paint(widget: &mut CalendarDatePickerWidget, size: Size) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(
        widget: &mut CalendarDatePickerWidget,
        app: &mut App,
        size: Size,
        event: &InputEvent,
    ) {
        let state_any: &mut dyn Any = app;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    /// Press and release at `(x, y)` — the up-inside gesture every affordance
    /// here fires on.
    fn tap(widget: &mut CalendarDatePickerWidget, app: &mut App, size: Size, x: f64, y: f64) {
        dispatch(widget, app, size, &ev(PointerPhase::Down, x, y));
        dispatch(widget, app, size, &ev(PointerPhase::Up, x, y));
    }

    /// The center of the day cell holding `day` in the displayed month.
    fn day_center(widget: &CalendarDatePickerWidget, day: u32) -> Point {
        let index = widget.grid().index_of(day).expect("a real day");
        widget.day_cell_rect(index).center()
    }

    // ---- geometry --------------------------------------------------------

    #[test]
    fn the_day_view_height_follows_the_displayed_month_row_count() {
        let state = DatePickerState::new(Some(d(2026, 2, 14)), d(2026, 2, 14));
        let (_, four) = laid_out(&view(state.clone()), 328.0);
        // February 2026 is a 4-row month; May 2026 a 6-row one.
        let (_, six) = laid_out(&view(state.with_displayed_month(d(2026, 5, 1))), 328.0);
        assert_eq!(four.height, 52.0 + 24.0 + 48.0 * 4.0 + 4.0);
        assert_eq!(six.height, 52.0 + 24.0 + 48.0 * 6.0 + 4.0);
        assert_eq!(four.width, 328.0);
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_reference_calendar_width() {
        let mut widget = build(&view(DatePickerState::new(None, d(2026, 8, 20))));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 900.0)),
        );
        assert_eq!(size.width, CALENDAR_WIDTH);
    }

    #[test]
    fn the_year_view_height_is_the_capped_grid_plus_the_sub_header() {
        let state = DatePickerState::new(Some(d(2026, 8, 20)), d(2026, 8, 20))
            .with_mode(DatePickerMode::Year);
        let (_, size) = laid_out(&view(state), 328.0);
        // 2020..=2030 is 11 years -> 4 rows -> 16 + 4*52 + 3*8 + 16 = 264.
        assert_eq!(size.height, SUB_HEADER_HEIGHT + 264.0);
    }

    #[test]
    fn the_nav_chevrons_sit_at_the_trailing_edge_and_vanish_in_the_year_view() {
        let (day, _) = laid_out(&view(DatePickerState::new(None, d(2026, 8, 20))), 328.0);
        let button = ARROW_ICON_SIZE + 2.0 * ARROW_PADDING;
        assert_eq!(day.geometry.next.x1, 328.0);
        assert_eq!(day.geometry.next.width(), button);
        assert_eq!(day.geometry.prev.x1, 328.0 - button);
        assert!(
            328.0 - day.geometry.prev.x0 <= MONTH_NAV_BUTTONS_WIDTH,
            "both affordances fit the reference's nav band"
        );

        let (year, _) = laid_out(
            &view(DatePickerState::new(None, d(2026, 8, 20)).with_mode(DatePickerMode::Year)),
            328.0,
        );
        assert_eq!(year.geometry.prev, Rect::ZERO);
        assert_eq!(year.geometry.next, Rect::ZERO);
    }

    #[test]
    fn the_day_grid_is_seven_equal_columns_under_the_weekday_band() {
        let (widget, _) = laid_out(&view(DatePickerState::new(None, d(2026, 5, 10))), 350.0);
        assert_eq!(widget.geometry.cell.width, 50.0);
        assert_eq!(widget.geometry.cell.height, DAY_ROW_HEIGHT);
        assert_eq!(
            widget.geometry.grid_origin.y,
            SUB_HEADER_HEIGHT + WEEKDAY_ROW_HEIGHT + DAY_GRID_TOP_PADDING
        );
        // May 2026 starts on a Friday: the 1st is in column 5 of row 0.
        let first = widget.grid().index_of(1).unwrap();
        assert_eq!(first, 5);
        assert_eq!(widget.day_cell_rect(first).x0, 5.0 * 50.0);
    }

    // ---- selection, bounds, today ----------------------------------------

    #[test]
    fn tapping_a_day_reports_it_selected() {
        let state = DatePickerState::new(None, d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 350.0);
        let mut app = App::default();
        let at = day_center(&widget, 14);
        tap(&mut widget, &mut app, size, at.x, at.y);
        assert_eq!(app.reported.len(), 1);
        assert_eq!(app.reported[0].selected, Some(d(2026, 5, 14)));
        assert_eq!(
            widget.state.selected, None,
            "a controlled widget never self-mutates"
        );
    }

    #[test]
    fn a_day_outside_the_bounds_is_not_hittable() {
        // Bounds inside a single month: only the 10th..=20th are selectable.
        let state = DatePickerState::new(None, d(2026, 5, 15));
        let picker = calendar_date_picker(
            state,
            d(2026, 5, 10),
            d(2026, 5, 20),
            |app: &mut App, next| app.reported.push(next),
        );
        let (mut widget, size) = laid_out(&picker, 350.0);
        let mut app = App::default();

        let outside = day_center(&widget, 3);
        tap(&mut widget, &mut app, size, outside.x, outside.y);
        assert!(
            app.reported.is_empty(),
            "an out-of-bounds day reports nothing"
        );

        let inside = day_center(&widget, 12);
        tap(&mut widget, &mut app, size, inside.x, inside.y);
        assert_eq!(app.reported.len(), 1);
        assert_eq!(app.reported[0].selected, Some(d(2026, 5, 12)));
    }

    #[test]
    fn a_refused_day_reports_nothing_even_inside_the_bounds() {
        let state = DatePickerState::new(None, d(2026, 5, 15));
        let picker = view(state).selectable(|date| date.day() % 2 == 0);
        let (mut widget, size) = laid_out(&picker, 350.0);
        let mut app = App::default();

        let odd = day_center(&widget, 7);
        tap(&mut widget, &mut app, size, odd.x, odd.y);
        assert!(app.reported.is_empty());

        let even = day_center(&widget, 8);
        tap(&mut widget, &mut app, size, even.x, even.y);
        assert_eq!(app.reported.len(), 1);
    }

    #[test]
    fn the_selected_day_fills_and_today_rings() {
        let state = DatePickerState::new(Some(d(2026, 5, 14)), d(2026, 5, 20));
        let (mut widget, size) = laid_out(&view(state).today(d(2026, 5, 20)), 350.0);
        let rec = paint(&mut widget, size);

        // Exactly one filled circle (the selection) and one ring (today). The
        // sub-header's own glyphs fill paths too, in their own ink, so the
        // count is over primary-inked fills.
        assert_eq!(
            primary_fills(&rec),
            1,
            "only the selected day fills a circle at rest"
        );
        assert_eq!(rec.strokes.len(), 1, "only today rings");
        assert_eq!(rec.strokes[0].1, TODAY_RING_WIDTH);
        assert_eq!(rec.strokes[0].2, FALLBACK_PRIMARY);
    }

    #[test]
    fn a_pressed_day_paints_a_state_layer_over_its_own_cell() {
        let state = DatePickerState::new(None, d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 350.0);
        let mut app = App::default();
        let rest = paint(&mut widget, size);

        let at = day_center(&widget, 14);
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Down, at.x, at.y),
        );
        let pressed = paint(&mut widget, size);
        assert_eq!(
            pressed.fills.len(),
            rest.fills.len() + 1,
            "the press adds exactly one overlay circle"
        );

        // Releasing clears it again.
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Up, at.x, at.y),
        );
        let released = paint(&mut widget, size);
        assert_eq!(released.fills.len(), rest.fills.len());
    }

    #[test]
    fn today_rings_only_when_the_app_supplies_it() {
        let state = DatePickerState::new(None, d(2026, 5, 1));
        let (mut widget, size) = laid_out(&view(state), 350.0);
        let rec = paint(&mut widget, size);
        assert!(
            rec.strokes.is_empty(),
            "no clock read means no ring without an explicit `today`"
        );
    }

    #[test]
    fn a_disabled_day_paints_at_the_reference_disabled_opacity() {
        let state = DatePickerState::new(None, d(2026, 5, 15));
        let picker = calendar_date_picker(
            state,
            d(2026, 5, 10),
            d(2026, 5, 20),
            |app: &mut App, next| app.reported.push(next),
        );
        let (mut widget, size) = laid_out(&picker, 350.0);
        let rec = paint(&mut widget, size);
        let dimmed = with_alpha(FALLBACK_ON_SURFACE, DISABLED_DAY_OPACITY);
        assert!(
            rec.runs.iter().any(|(_, color, _)| *color == dimmed),
            "days outside the bounds paint dimmed"
        );
        assert!(
            rec.runs
                .iter()
                .any(|(_, color, _)| *color == FALLBACK_ON_SURFACE),
            "days inside the bounds paint at full ink"
        );
    }

    #[test]
    fn a_complete_range_fills_its_span_and_caps_both_ends() {
        let state = DatePickerState::new(None, d(2026, 5, 1));
        let picker = view(state).range(DateRange::closed(d(2026, 5, 5), d(2026, 5, 9)));
        let (mut widget, size) = laid_out(&picker, 350.0);
        let rec = paint(&mut widget, size);
        let band = with_alpha(FALLBACK_PRIMARY, RANGE_HIGHLIGHT_ALPHA);
        let bands = rec.rects.iter().filter(|(_, _, c)| *c == band).count();
        assert_eq!(bands, 5, "one band per in-range day, 5th through 9th");
        assert_eq!(primary_fills(&rec), 2, "both caps fill a circle");
    }

    // ---- month paging -----------------------------------------------------

    #[test]
    fn the_chevrons_page_one_month_and_disable_at_the_bounds() {
        let state = DatePickerState::new(Some(d(2026, 5, 10)), d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 328.0);
        let mut app = App::default();

        let next = widget.geometry.next.center();
        tap(&mut widget, &mut app, size, next.x, next.y);
        assert_eq!(app.reported.pop().unwrap().displayed_month, d(2026, 6, 1));

        let prev = widget.geometry.prev.center();
        tap(&mut widget, &mut app, size, prev.x, prev.y);
        assert_eq!(app.reported.pop().unwrap().displayed_month, d(2026, 4, 1));

        // At the first month, stepping back is refused.
        let at_start = DatePickerState::new(Some(d(2020, 1, 5)), d(2020, 1, 5));
        assert!(!at_start.can_step_month(-1, FIRST(), LAST()));
        assert!(at_start.can_step_month(1, FIRST(), LAST()));
    }

    #[test]
    fn a_month_change_slides_and_settles() {
        let state = DatePickerState::new(Some(d(2026, 5, 10)), d(2026, 5, 10));
        let before = view(state.clone());
        let (mut widget, size) = laid_out(&before, 328.0);

        let after = view(state.stepped_month(1, FIRST(), LAST()));
        let mut counter = 0u64;
        View::<App>::rebuild(
            &after,
            &before,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(widget.slide_from, Some(d(2026, 5, 1)));
        assert_eq!(widget.slide_dir, 1);

        // The first paint only seeds the controller's clock.
        layout_at(&mut widget, 328.0);
        let mut seed = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, FrameTime::ZERO);
        widget.paint(&mut pctx, &mut seed);
        assert_eq!(seed.clips, 0, "the grid clip is balanced");

        // Mid-flight both pages paint.
        let mut mid = Recorder::default();
        let mut pctx = PaintCtx::for_test(
            Point::ZERO,
            size,
            FrameTime::from_nanos(MONTH_SCROLL_DURATION.as_nanos() as u64 / 2),
        );
        widget.paint(&mut pctx, &mut mid);
        assert_ne!(widget.slide_dir, 0, "still sliding at half the duration");

        // Past the duration the slide settles and only one month remains.
        let mut done = Recorder::default();
        let mut pctx = PaintCtx::for_test(
            Point::ZERO,
            size,
            FrameTime::from_nanos(MONTH_SCROLL_DURATION.as_nanos() as u64 * 3),
        );
        widget.paint(&mut pctx, &mut done);
        assert_eq!(widget.slide_dir, 0);
        assert_eq!(widget.slide_from, None);
        assert!(
            mid.runs.len() > done.runs.len(),
            "both pages' day numbers paint mid-slide ({} vs {} settled)",
            mid.runs.len(),
            done.runs.len()
        );
        assert_eq!(done.clips, 0);
    }

    #[test]
    fn reduce_motion_skips_the_slide_entirely() {
        let state = DatePickerState::new(Some(d(2026, 5, 10)), d(2026, 5, 10));
        let before = view(state.clone());
        let (mut widget, size) = laid_out(&before, 328.0);
        let after = view(state.stepped_month(1, FIRST(), LAST()));
        let mut counter = 0u64;
        View::<App>::rebuild(
            &after,
            &before,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(widget.slide_dir, 1);

        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme as &dyn Any);
        widget.paint(&mut pctx, &mut rec);
        assert_eq!(widget.slide_dir, 0, "reduce_motion settles immediately");
    }

    // ---- year selector ----------------------------------------------------

    #[test]
    fn the_sub_header_toggles_the_calendar_sub_view() {
        let state = DatePickerState::new(Some(d(2026, 5, 10)), d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 328.0);
        let mut app = App::default();
        let at = widget.geometry.toggle.center();
        tap(&mut widget, &mut app, size, at.x, at.y);
        assert_eq!(app.reported.pop().unwrap().mode, DatePickerMode::Year);
    }

    #[test]
    fn tapping_a_year_reports_the_candidate_and_returns_to_the_day_grid() {
        let state = DatePickerState::new(Some(d(2026, 5, 10)), d(2026, 5, 10))
            .with_mode(DatePickerMode::Year);
        let (mut widget, size) = laid_out(&view(state), 328.0);
        let mut app = App::default();
        // 2020 is the first cell of the first row.
        let at = widget.year_cell_rect(0).center();
        tap(&mut widget, &mut app, size, at.x, at.y);
        let next = app.reported.pop().expect("a year tap reports");
        assert_eq!(next.mode, DatePickerMode::Day);
        assert_eq!(next.selected, Some(d(2020, 5, 10)));
        assert_eq!(next.displayed_month, d(2020, 5, 1));
    }

    #[test]
    fn the_year_grid_seeds_its_scroll_at_the_selected_year() {
        // A span long enough to overflow the capped viewport.
        let state = DatePickerState::new(Some(d(2075, 6, 1)), d(2075, 6, 1))
            .with_mode(DatePickerMode::Year);
        let picker = calendar_date_picker(
            state,
            d(2000, 1, 1),
            d(2099, 12, 31),
            |app: &mut App, next| app.reported.push(next),
        );
        let (widget, _) = laid_out(&picker, 328.0);
        assert!(widget.max_year_scroll() > 0.0, "100 years overflow the cap");
        // 2075 is index 75 -> row 25 -> 25 * (52 + 8).
        assert_eq!(
            widget.year_scroll,
            (25.0 * 60.0f64).min(widget.max_year_scroll())
        );
    }

    #[test]
    fn the_year_grid_scrolls_on_a_wheel_and_clamps_at_both_ends() {
        let state = DatePickerState::new(Some(d(2000, 6, 1)), d(2000, 6, 1))
            .with_mode(DatePickerMode::Year);
        let picker = calendar_date_picker(
            state,
            d(2000, 1, 1),
            d(2099, 12, 31),
            |app: &mut App, next| app.reported.push(next),
        );
        let (mut widget, size) = laid_out(&picker, 328.0);
        let mut app = App::default();
        assert_eq!(widget.year_scroll, 0.0);

        let inside = widget.geometry.year_viewport.center();
        let scroll = |dy: f64| InputEvent::Scroll {
            position: inside,
            delta: ScrollDelta::Lines(0.0, dy),
        };
        dispatch(&mut widget, &mut app, size, &scroll(-2.0));
        assert_eq!(widget.year_scroll, 2.0 * YEAR_WHEEL_LINE);

        // Scrolling back past the top clamps at zero, never negative.
        dispatch(&mut widget, &mut app, size, &scroll(50.0));
        assert_eq!(widget.year_scroll, 0.0);
        // And past the end clamps at the maximum.
        dispatch(&mut widget, &mut app, size, &scroll(-500.0));
        assert_eq!(widget.year_scroll, widget.max_year_scroll());
        assert!(app.reported.is_empty(), "scrolling never selects");
    }

    #[test]
    fn a_year_drag_past_the_slop_scrolls_instead_of_selecting() {
        let state = DatePickerState::new(Some(d(2000, 6, 1)), d(2000, 6, 1))
            .with_mode(DatePickerMode::Year);
        let picker = calendar_date_picker(
            state,
            d(2000, 1, 1),
            d(2099, 12, 31),
            |app: &mut App, next| app.reported.push(next),
        );
        let (mut widget, size) = laid_out(&picker, 328.0);
        let mut app = App::default();
        let at = widget.year_cell_rect(0).center();

        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Down, at.x, at.y),
        );
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Move, at.x, at.y - TOUCH_SLOP - 10.0),
        );
        assert!(widget.year_scroll > 0.0, "the drag scrolled");
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Up, at.x, at.y - TOUCH_SLOP - 10.0),
        );
        assert!(app.reported.is_empty(), "a drag never selects a year");
    }

    // ---- interaction discipline ------------------------------------------

    #[test]
    fn only_a_primary_press_starts_a_gesture() {
        let state = DatePickerState::new(None, d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 350.0);
        let mut app = App::default();
        let at = day_center(&widget, 14);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: at,
            button: PointerButton::Secondary,
        });
        dispatch(&mut widget, &mut app, size, &secondary);
        assert!(!widget.captured);
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Up, at.x, at.y),
        );
        assert!(app.reported.is_empty());
    }

    #[test]
    fn a_release_outside_the_pressed_cell_fires_nothing() {
        let state = DatePickerState::new(None, d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 350.0);
        let mut app = App::default();
        let from = day_center(&widget, 14);
        let to = day_center(&widget, 20);
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Down, from.x, from.y),
        );
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Move, to.x, to.y),
        );
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Up, to.x, to.y),
        );
        assert!(app.reported.is_empty(), "fire on up-inside only");
    }

    #[test]
    fn a_cancel_clears_the_press_without_touching_state() {
        let state = DatePickerState::new(None, d(2026, 5, 10));
        let (mut widget, size) = laid_out(&view(state), 350.0);
        // A synthesized `Cancel` can arrive over a throwaway `()` state, which
        // is exactly why the arm may not reach for `state_mut`.
        let at = day_center(&widget, 14);
        let mut app = App::default();
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Down, at.x, at.y),
        );
        assert!(widget.captured);
        let mut unit = ();
        let state_any: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, &ev(PointerPhase::Cancel, at.x, at.y));
        assert!(!widget.captured);
        assert_eq!(widget.pressed, None);
        assert!(app.reported.is_empty());
    }

    #[test]
    fn a_press_reports_exactly_once_per_gesture() {
        let calls = StdRc::new(RefCell::new(0usize));
        let counter = calls.clone();
        let state = DatePickerState::new(None, d(2026, 5, 10));
        let picker = calendar_date_picker(state, FIRST(), LAST(), move |_: &mut App, _| {
            *counter.borrow_mut() += 1;
        });
        let (mut widget, size) = laid_out(&picker, 350.0);
        let mut app = App::default();
        let at = day_center(&widget, 14);
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Down, at.x, at.y),
        );
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Move, at.x, at.y),
        );
        dispatch(
            &mut widget,
            &mut app,
            size,
            &ev(PointerPhase::Up, at.x, at.y),
        );
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn the_selected_year_fills_a_pill() {
        let state = DatePickerState::new(Some(d(2026, 5, 10)), d(2026, 5, 10))
            .with_mode(DatePickerMode::Year);
        let (mut widget, size) = laid_out(&view(state), 328.0);
        let rec = paint(&mut widget, size);
        let selected: Vec<_> = rec
            .rrects
            .iter()
            .filter(|(_, _, _, c)| *c == FALLBACK_PRIMARY)
            .collect();
        assert_eq!(selected.len(), 1, "only the selected year fills");
        assert_eq!(selected[0].1.height, YEAR_ROW_HEIGHT);
        assert_eq!(selected[0].2, YEAR_ROW_HEIGHT / 2.0, "a fully rounded pill");
        assert_eq!(rec.clips, 0, "the year viewport clip is balanced");
    }

    // ---- semantics --------------------------------------------------------

    #[test]
    fn every_real_day_contributes_its_own_labelled_node() {
        // `SemanticsCtx` is only constructible inside `frust-core`, so this
        // drives a real `RenderRoot` pass (the route `crate::switch`'s own
        // semantics tests take).
        fn logic(_app: &mut App) -> CalendarDatePicker<App> {
            calendar_date_picker(
                DatePickerState::new(None, MaterialDate::new(2026, 5, 10)),
                MaterialDate::new(2026, 5, 10),
                MaterialDate::new(2026, 5, 20),
                |app: &mut App, next| app.reported.push(next),
            )
        }
        let mut root: frust_core::RenderRoot<App, CalendarDatePicker<App>> =
            frust_core::RenderRoot::new();
        let mut app = App::default();
        root.rebuild(&mut logic, &mut app);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(350.0, 700.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 31, "one node per real day in May 2026");
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Group),
            "the grid contributes a container node"
        );

        // The 12th is inside the bounds and clickable; the 3rd is not.
        let node_for = |label: &str| {
            buttons
                .iter()
                .find(|(_, n)| n.label() == Some(label))
                .map(|(_, n)| n)
                .unwrap_or_else(|| panic!("no node labelled {label}"))
        };
        assert!(node_for("12").supports_action(Action::Click));
        assert!(!node_for("3").supports_action(Action::Click));
        assert!(node_for("3").is_disabled());
        // Every cell carries its own bounds, not the grid's.
        let twelfth = node_for("12").bounds().expect("a day node has bounds");
        assert!(twelfth.x1 - twelfth.x0 < 350.0);
    }
}
