// Ported from `material_3_expressive` v1.0.8's date-picker family (MIT, © 2026
// Paa Developments; `tmp/material_3_expressive/lib/components/date_pickers/` —
// `m3e_date_pickers.dart`, `m3e_calendar_date_picker.dart`,
// `m3e_date_picker_dialog.dart`, `m3e_date_range_picker_dialog.dart`,
// `components/{m3e_day_cell, m3e_day_picker, m3e_month_picker, m3e_year_picker,
// m3e_date_picker_header, m3e_date_picker_actions, m3e_date_picker_mode_toggle,
// m3e_date_picker_dialog_content, m3e_input_date_picker_form_field,
// m3e_input_date_range_picker_form_field, m3e_calendar_date_range_picker}.dart`,
// `enums/`, `models/`, `res/`, `styles/`, `utils/`, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (each documented, with its reference site, in the module
// docs below): the civil-calendar math is implemented in-crate rather than
// taken from a date library; localization is a caller-supplied
// `DatePickerStrings` struct with English/US defaults in place of Flutter's
// `MaterialLocalizations`; the picker is a *controlled* component driven by one
// `DatePickerState` value rather than the reference's `StatefulWidget` local
// state; the landscape dialog layout and the scrolling multi-month **range**
// dialog are v1 cuts.

//! The M3 Expressive **date picker**: a calendar month grid with a year
//! selector, a text-input mode, and the modal dialog that presents either.
//!
//! - [`mod@date`] — [`MaterialDate`], the whole-day value type, and its
//!   civil-calendar arithmetic; [`DateRange`], the range model.
//! - [`mod@grid`] — the pure day/year grid geometry (leading blanks, row
//!   counts, sub-view heights).
//! - [`mod@calendar`] — [`calendar_date_picker`], the embeddable calendar:
//!   month header with prev/next and a year-selector toggle, the 7-column day
//!   grid, the scrolling year grid, and the month-paging slide.
//! - [`mod@input`] — [`date_input_field`], the input mode's text field with
//!   format/range validation over [`crate::text_field`].
//! - [`mod@dialog`] — [`date_picker_dialog`]/[`show_date_picker`], the modal
//!   presentation on the shared [`crate::overlay::modal`] host.
//!
//! # Controlled, through one [`DatePickerState`] value
//!
//! The reference's `M3ECalendarDatePicker`/`M3EDatePickerDialog` are
//! `StatefulWidget`s owning `_selectedDate`, `_displayedMonth`, `_mode` and
//! `_entryMode` in their own `State`. This catalog's rule is the opposite —
//! *controlled components never self-mutate* (`docs/CODE_STANDARDS.md`'s
//! Interaction Semantics), the same stance [`crate::dialog`]'s selection dialog
//! already takes — so all four live in one [`DatePickerState`] the app holds,
//! and every widget here **computes and reports** the next state rather than
//! writing its own:
//!
//! ```ignore
//! calendar_date_picker(state.picker.clone(), first, last, |state: &mut App, next| {
//!     state.picker = next;
//! })
//! ```
//!
//! One prop and one callback rather than five of each: the transitions
//! ([`DatePickerState::with_selected`], [`DatePickerState::stepped_month`],
//! [`DatePickerState::with_year`], [`DatePickerState::toggled_entry_mode`], …)
//! are plain, total functions on the value, so an app's `on_change` is a single
//! assignment and the interaction rules are pinned as pure-function tests
//! rather than inferred from a painted scene.
//!
//! # Localization: English defaults plus an override struct
//!
//! Every string and the first-day-of-week index come from
//! [`DatePickerStrings`], which defaults to [`DatePickerStrings::ENGLISH`] —
//! Flutter `MaterialLocalizations`' own `en_US` values, which is what the
//! reference reads (`localizations.formatMonthYear`, `.narrowWeekdays`,
//! `.firstDayOfWeekIndex`, `.datePickerHelpText`, `.invalidDateFormatLabel`, …)
//! and what its own `M3ECalendarLabels` hard-codes as a fallback.
//!
//! **This is an adaptation, not a port of the localization itself.** This crate
//! has no i18n seam (a design-system plugin may not depend on a sibling plugin
//! — `docs/PLUGINS_ARCHITECTURE.md`'s Layer Dependencies — so `frust-i18n` is
//! out of reach), and the input mode's `mm/dd/yyyy` parse/format
//! ([`MaterialDate::format_compact`]) is likewise the `en_US` compact form
//! rather than a locale-resolved one. An app that ships other languages
//! substitutes its own [`DatePickerStrings`] (`..DatePickerStrings::ENGLISH`
//! for the fields it does not translate) and reformats dates itself; a
//! non-`mm/dd/yyyy` *input* format is not overridable in v1.
//!
//! # Deliberate v1 scope cuts
//!
//! - **No landscape dialog layout.** The reference switches
//!   `M3EDatePickerDialog` between a portrait column and a landscape row with a
//!   side header (`calendarLandscapeDialogSize`, `headerLandscapeWidth`),
//!   keyed off `MediaQuery.orientationOf`. This port ships the portrait shape
//!   only, at every window size — the same single-shape choice
//!   [`crate::dialog`] makes.
//! - **No range *dialog*.** `M3EDateRangePickerDialog` presents a vertically
//!   scrolling list of every month between the bounds; that is a second, larger
//!   surface with its own scroll physics, which this deep in a modal's content
//!   tree has no viewport to lean on (the limit [`crate::dialog`]'s selection
//!   list records). The range **model** ([`DateRange`]) and the day cell's
//!   range visuals ([`CalendarDatePicker::range`]) do ship, so an app can drive
//!   range selection against the single-month calendar.
//! - **No text-scale clamping.** The reference caps its own layout against
//!   `MediaQuery.textScalerOf` (`maxTextScaleFactor`, `maxHeaderTextScaleFactor`,
//!   …); the framework exposes no text-scale factor to a widget, so those
//!   constants have nothing to clamp here.
//! - **No keyboard month paging.** `_M3EMonthPickerState._handleKey` pages on
//!   arrow-left/right while the grid holds focus; this widget claims no focus
//!   of its own (it has no editable and no focusable child), so there is no
//!   focus path for a `Key` event to reach it on.
//!
//! # Geometry (cited from `m3e_date_picker_constants.dart` / `m3e_date_picker_theme.dart`)
//!
//! [`SUB_HEADER_HEIGHT`] (52), [`WEEKDAY_ROW_HEIGHT`] (24), [`DAY_ROW_HEIGHT`]
//! (48), [`DAY_GRID_TOP_PADDING`] (4), [`MONTH_NAV_BUTTONS_WIDTH`] (108),
//! [`YEAR_ROW_HEIGHT`] (52), [`YEAR_ROW_SPACING`] (8), [`YEAR_GRID_PADDING`]
//! (16), [`YEAR_COLUMN_COUNT`] (3), [`MAX_DAY_PICKER_HEIGHT`] (48 × 7),
//! [`CALENDAR_WIDTH`] (328), [`CALENDAR_PADDING`] (12), [`DAY_SIZE`] (40),
//! [`ARROW_ICON_SIZE`] (24), [`ARROW_PADDING`] (8), [`HEADER_PORTRAIT_HEIGHT`]
//! (120), [`ACTIONS_MIN_HEIGHT`] (52), [`MONTH_SCROLL_DURATION`] (200ms),
//! [`DIALOG_PORTRAIT_CALENDAR_WIDTH`] (360), [`DIALOG_PORTRAIT_INPUT_WIDTH`]
//! (328).

use std::time::Duration;

use frust::authoring::TypedArgCallback;

pub mod calendar;
pub mod date;
pub mod dialog;
pub mod grid;
pub mod input;

pub use calendar::{CalendarDatePicker, CalendarDatePickerWidget, calendar_date_picker};
pub use date::{DateRange, MAX_YEAR, MIN_YEAR, MaterialDate, days_in_month, is_leap_year};
pub use dialog::{DatePickerDialog, date_picker_dialog, show_date_picker};
pub use grid::MonthGrid;
pub use input::{
    DateInputError, DateInputField, DateInputFieldWidget, date_input_field, parse_bounded,
};

/// The one callback every view in this family reports through: "here is the
/// state you should now hold". See the [module docs](self)' controlled note.
pub type OnDatePickerChange<State> = TypedArgCallback<State, DatePickerState>;

// ---- Layout constants (m3e_date_picker_constants.dart / m3e_date_picker_theme.dart) ----

/// Columns in the day grid (`M3EDatePickerTheme.daysPerWeek`). A `usize`
/// because every consumer indexes a grid with it.
pub const DAYS_PER_WEEK: usize = 7;

/// The month/year sub-header band's height, in logical px (`subHeaderHeight`).
pub const SUB_HEADER_HEIGHT: f64 = 52.0;

/// The weekday-initial header row's height, in logical px (`weekdayRowHeight`).
pub const WEEKDAY_ROW_HEIGHT: f64 = 24.0;

/// One week row's height in the day grid, in logical px (`dayPickerRowHeight`).
pub const DAY_ROW_HEIGHT: f64 = 48.0;

/// The day grid's own top inset, in logical px (`dayGridTopPadding` /
/// `M3EDatePickerTheme.gridPadding`'s `top: 4`).
pub const DAY_GRID_TOP_PADDING: f64 = 4.0;

/// The most week rows any month occupies (`maxDayPickerRowCount`).
pub const MAX_DAY_PICKER_ROW_COUNT: usize = 6;

/// The height ceiling a scrollable sub-view is capped at, in logical px
/// (`maxDayPickerHeight`, `dayPickerRowHeight * (maxDayPickerRowCount + 1)`).
pub const MAX_DAY_PICKER_HEIGHT: f64 = DAY_ROW_HEIGHT * (MAX_DAY_PICKER_ROW_COUNT as f64 + 1.0);

/// The width the prev/next month affordances share at the sub-header's trailing
/// edge, in logical px (`monthNavButtonsWidth`).
pub const MONTH_NAV_BUTTONS_WIDTH: f64 = 108.0;

/// The sub-header's leading inset, in logical px — the reference's
/// `EdgeInsetsDirectional.only(start: 16)` on both the month/year label and the
/// year sub-view's own toggle.
pub const SUB_HEADER_START_INSET: f64 = 16.0;

/// Columns in the year grid (`yearPickerColumnCount`).
pub const YEAR_COLUMN_COUNT: usize = 3;

/// The year grid's inset on all four edges, in logical px (`yearPickerPadding`).
pub const YEAR_GRID_PADDING: f64 = 16.0;

/// One year cell's height, in logical px (`yearPickerRowHeight`).
pub const YEAR_ROW_HEIGHT: f64 = 52.0;

/// The gap between year cells, both axes, in logical px
/// (`yearPickerRowSpacing`).
pub const YEAR_ROW_SPACING: f64 = 8.0;

/// The inline calendar's own width, in logical px (`M3EDatePickerTheme.width`)
/// — the fallback this widget sizes to when its constraints are unbounded.
pub const CALENDAR_WIDTH: f64 = 328.0;

/// The inline calendar's padding on all four edges, in logical px
/// (`M3EDatePickerTheme.padding`).
pub const CALENDAR_PADDING: f64 = 12.0;

/// One day cell's selected/today circle diameter, in logical px
/// (`M3EDatePickerTheme.daySize`) — smaller than [`DAY_ROW_HEIGHT`], which is
/// the cell's own touch-target box.
pub const DAY_SIZE: f64 = 40.0;

/// The sub-header's chevron/drop-arrow glyph size, in logical px
/// (`M3EDatePickerTheme.arrowIconSize`).
pub const ARROW_ICON_SIZE: f64 = 24.0;

/// The padding around each sub-header chevron, in logical px
/// (`M3EDatePickerTheme.arrowPadding`).
pub const ARROW_PADDING: f64 = 8.0;

/// The dialog header band's height, in logical px (`headerPortraitHeight`).
pub const HEADER_PORTRAIT_HEIGHT: f64 = 120.0;

/// The dialog action row's minimum height, in logical px (`actionsMinHeight`).
pub const ACTIONS_MIN_HEIGHT: f64 = 52.0;

/// The month-paging slide's duration (`monthScrollDuration`).
pub const MONTH_SCROLL_DURATION: Duration = Duration::from_millis(200);

/// The dialog's portrait width in calendar mode, in logical px
/// (`calendarPortraitDialogSize.width`).
pub const DIALOG_PORTRAIT_CALENDAR_WIDTH: f64 = 360.0;

/// The dialog's portrait width in input mode, in logical px
/// (`inputPortraitDialogSize.width`).
pub const DIALOG_PORTRAIT_INPUT_WIDTH: f64 = 328.0;

/// The opacity a disabled day/year cell's ink composites at (0.38) — the
/// reference's `M3EColorUtils.withOpacity(scheme.onSurface, 0.38)` in
/// `dayForegroundColor`/`yearForegroundColor`.
pub const DISABLED_DAY_OPACITY: f32 = 0.38;

/// The alpha a range highlight fills at (0.12) — `rangeHighlightColor`'s
/// `withOpacity(scheme.primary, 0.12)`.
pub const RANGE_HIGHLIGHT_ALPHA: f32 = 0.12;

// ---- Enums (enums/m3e_date_picker_enums.dart) --------------------------------

/// Which calendar sub-view is showing — the reference's `M3EDatePickerMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DatePickerMode {
    /// The day grid for the displayed month. Default.
    #[default]
    Day,
    /// The scrollable year grid.
    Year,
}

impl DatePickerMode {
    /// The other sub-view — what the sub-header's label toggles to.
    pub fn toggled(self) -> Self {
        match self {
            DatePickerMode::Day => DatePickerMode::Year,
            DatePickerMode::Year => DatePickerMode::Day,
        }
    }
}

/// How the dialog takes its date — the reference's `M3EDatePickerEntryMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DatePickerEntryMode {
    /// Calendar, with an affordance to switch to text input. Default.
    #[default]
    Calendar,
    /// Text input, with an affordance to switch to the calendar.
    Input,
    /// Calendar only — no switch affordance at all.
    CalendarOnly,
    /// Text input only — no switch affordance at all.
    InputOnly,
}

impl DatePickerEntryMode {
    /// Whether this mode shows the text field rather than the calendar.
    pub fn is_input(self) -> bool {
        matches!(
            self,
            DatePickerEntryMode::Input | DatePickerEntryMode::InputOnly
        )
    }

    /// Whether the header offers a switch affordance — false for the two
    /// `*Only` modes, which the reference gives no entry-mode button.
    pub fn is_toggleable(self) -> bool {
        matches!(
            self,
            DatePickerEntryMode::Calendar | DatePickerEntryMode::Input
        )
    }

    /// The mode a tap on the switch affordance requests. A `*Only` mode has no
    /// affordance and reports itself unchanged, so a caller that wires the
    /// toggle unconditionally still cannot escape a locked mode — the
    /// reference's `case calendarOnly: case inputOnly: break;`.
    pub fn toggled(self) -> Self {
        match self {
            DatePickerEntryMode::Calendar => DatePickerEntryMode::Input,
            DatePickerEntryMode::Input => DatePickerEntryMode::Calendar,
            other => other,
        }
    }
}

// ---- Strings (models/m3e_calendar_labels.dart + MaterialLocalizations en_US) --

/// Every user-visible string and the first-day-of-week index the date-picker
/// family reads.
///
/// `Copy` and `&'static str`-valued, so a widget holds one by value with no
/// allocation. Override by struct update:
///
/// ```
/// use frust_material::DatePickerStrings;
///
/// const FRENCH: DatePickerStrings = DatePickerStrings {
///     months: [
///         "janvier", "février", "mars", "avril", "mai", "juin",
///         "juillet", "août", "septembre", "octobre", "novembre", "décembre",
///     ],
///     first_day_of_week: 1, // Monday
///     cancel_label: "Annuler",
///     ..DatePickerStrings::ENGLISH
/// };
/// assert_eq!(FRENCH.months[0], "janvier");
/// assert_eq!(FRENCH.confirm_label, "OK");
/// ```
///
/// See the [module docs](self)' localization note for why this replaces the
/// reference's `MaterialLocalizations` lookup, and what it does *not* cover
/// (the `mm/dd/yyyy` input format).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePickerStrings {
    /// Full month names, January first (`M3ECalendarLabels.months`).
    pub months: [&'static str; 12],
    /// Abbreviated month names, January first — the `MMM` field of
    /// [`Self::medium_date`].
    pub month_abbreviations: [&'static str; 12],
    /// One-letter weekday headers, **Sunday first** regardless of
    /// [`Self::first_day_of_week`] (`M3ECalendarLabels.weekdayInitials` /
    /// `MaterialLocalizations.narrowWeekdays`).
    pub weekday_initials: [&'static str; 7],
    /// Abbreviated weekday names, **Sunday first** — the `EEE` field of
    /// [`Self::medium_date`].
    pub weekday_abbreviations: [&'static str; 7],
    /// Which weekday a week starts on: `0` Sunday .. `6` Saturday
    /// (`MaterialLocalizations.firstDayOfWeekIndex`).
    pub first_day_of_week: u32,
    /// The dialog header's help line (`datePickerHelpText`).
    pub help_text: &'static str,
    /// The dialog headline shown before any date is picked
    /// (`unspecifiedDate`).
    pub no_date_selected: &'static str,
    /// The dialog's dismiss action (`cancelButtonLabel`).
    pub cancel_label: &'static str,
    /// The dialog's confirm action (`okButtonLabel`).
    pub confirm_label: &'static str,
    /// The input field's label (`dateInputLabel`).
    pub field_label: &'static str,
    /// The input field's supporting/hint line (`dateHelpText`).
    pub field_hint: &'static str,
    /// The error for text that is not a date at all
    /// (`invalidDateFormatLabel`).
    pub error_format: &'static str,
    /// The error for a real date outside the selectable bounds
    /// (`invalidDateRangeLabel`).
    pub error_invalid: &'static str,
    /// The previous-month affordance's accessibility label
    /// (`previousMonthTooltip`).
    pub previous_month: &'static str,
    /// The next-month affordance's accessibility label (`nextMonthTooltip`).
    pub next_month: &'static str,
    /// The year-selector toggle's accessibility label
    /// (`selectYearSemanticsLabel`).
    pub select_year: &'static str,
    /// The switch-to-input affordance's accessibility label
    /// (`inputDateModeButtonLabel`).
    pub input_mode: &'static str,
    /// The switch-to-calendar affordance's accessibility label
    /// (`calendarModeButtonLabel`).
    pub calendar_mode: &'static str,
}

impl DatePickerStrings {
    /// Flutter `MaterialLocalizations`' own `en_US` values — what the reference
    /// resolves on an English device, and what
    /// [`M3ECalendarLabels`](self)'s hard-coded fallback table already carries.
    pub const ENGLISH: Self = Self {
        months: [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ],
        month_abbreviations: [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ],
        weekday_initials: ["S", "M", "T", "W", "T", "F", "S"],
        weekday_abbreviations: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
        first_day_of_week: 0,
        help_text: "Select date",
        no_date_selected: "Date",
        cancel_label: "Cancel",
        confirm_label: "OK",
        field_label: "Enter Date",
        field_hint: "mm/dd/yyyy",
        error_format: "Invalid format.",
        error_invalid: "Out of range.",
        previous_month: "Previous month",
        next_month: "Next month",
        select_year: "Select year",
        input_mode: "Switch to text input mode",
        calendar_mode: "Switch to calendar mode",
    };

    /// The header initial for grid column `column` (`0` = the leftmost), which
    /// [`Self::first_day_of_week`] rotates — the reference's
    /// `narrowWeekdays[(firstDayOfWeekIndex + i) % 7]`.
    pub fn weekday_initial(&self, column: usize) -> &'static str {
        self.weekday_initials[(self.first_day_of_week as usize + column) % DAYS_PER_WEEK]
    }

    /// `"March 2026"` — `MaterialLocalizations.formatMonthYear`.
    pub fn month_year(&self, date: MaterialDate) -> String {
        format!(
            "{} {}",
            self.months[(date.month() - 1) as usize],
            date.year()
        )
    }

    /// `"2026"` — `MaterialLocalizations.formatYear`.
    pub fn year(&self, year: i32) -> String {
        year.to_string()
    }

    /// `"Thu, Aug 20"` — `MaterialLocalizations.formatMediumDate`'s `en_US`
    /// `EEE, MMM d` pattern, which is what the dialog headline shows.
    pub fn medium_date(&self, date: MaterialDate) -> String {
        format!(
            "{}, {} {}",
            self.weekday_abbreviations[date.weekday_index() as usize],
            self.month_abbreviations[(date.month() - 1) as usize],
            date.day()
        )
    }
}

impl Default for DatePickerStrings {
    fn default() -> Self {
        Self::ENGLISH
    }
}

// ---- The controlled state ----------------------------------------------------

/// The date picker's whole controlled state: what is selected, which month and
/// sub-view the calendar shows, which entry mode the dialog is in, and the
/// input field's draft text.
///
/// Held by the app, never by a widget — see the [module docs](self)' controlled
/// note. Every `with_*`/`stepped_*`/`toggled_*` method below returns a **new**
/// value rather than mutating, which is exactly the shape a widget's
/// `on_change` callback reports and an app's handler assigns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatePickerState {
    /// The picked date, or `None` if nothing is picked yet.
    pub selected: Option<MaterialDate>,
    /// The month the day grid shows, normalized to its 1st.
    pub displayed_month: MaterialDate,
    /// Which calendar sub-view is showing.
    pub mode: DatePickerMode,
    /// Which entry mode the dialog is in (ignored by a bare
    /// [`calendar_date_picker`], which is always a calendar).
    pub entry_mode: DatePickerEntryMode,
    /// The input field's current text. Kept in sync with [`Self::selected`] by
    /// [`Self::with_selected`], and free-form while the user types.
    pub input_text: String,
}

impl DatePickerState {
    /// A fresh state: `initial` selected (or nothing), showing that date's
    /// month — or `today`'s if nothing is selected — in the day sub-view and
    /// the calendar entry mode.
    ///
    /// Mirrors `_M3ECalendarDatePickerState.initState`'s
    /// `base = _selectedDate ?? _currentDate; _displayedMonth = getMonth(base)`.
    pub fn new(initial: Option<MaterialDate>, today: MaterialDate) -> Self {
        let base = initial.unwrap_or(today);
        Self {
            selected: initial,
            displayed_month: base.first_of_month(),
            mode: DatePickerMode::Day,
            entry_mode: DatePickerEntryMode::Calendar,
            input_text: initial
                .map(MaterialDate::format_compact)
                .unwrap_or_default(),
        }
    }

    /// This state with `selected` picked: the date recorded, the displayed
    /// month moved onto it, and [`Self::input_text`] refreshed to its compact
    /// form so a later switch into input mode shows the picked date (the
    /// reference's `_updateValueForSelectedDate`).
    pub fn with_selected(&self, date: MaterialDate) -> Self {
        Self {
            selected: Some(date),
            displayed_month: date.first_of_month(),
            input_text: date.format_compact(),
            ..self.clone()
        }
    }

    /// This state with the displayed month replaced (normalized to its 1st).
    /// The selection is untouched — paging past the selected month keeps it
    /// selected, exactly as the reference's `PageView` does.
    pub fn with_displayed_month(&self, month: MaterialDate) -> Self {
        Self {
            displayed_month: month.first_of_month(),
            ..self.clone()
        }
    }

    /// This state paged `delta` months, clamped so the displayed month never
    /// leaves `first..=last`'s own months — the reference's page-count bound
    /// (`page > 0` / `page < _monthCount - 1`).
    pub fn stepped_month(&self, delta: i32, first: MaterialDate, last: MaterialDate) -> Self {
        let low = first.first_of_month();
        let high = last.first_of_month();
        let next = self.displayed_month.add_months(delta).clamp_to(low, high);
        self.with_displayed_month(next)
    }

    /// Whether paging by `delta` would actually move — what the prev/next
    /// affordances enable off.
    pub fn can_step_month(&self, delta: i32, first: MaterialDate, last: MaterialDate) -> bool {
        self.stepped_month(delta, first, last).displayed_month != self.displayed_month
    }

    /// This state showing calendar sub-view `mode`.
    pub fn with_mode(&self, mode: DatePickerMode) -> Self {
        Self {
            mode,
            ..self.clone()
        }
    }

    /// This state with the calendar sub-view toggled (day ⇄ year) — what the
    /// sub-header's month/year label reports.
    pub fn toggled_mode(&self) -> Self {
        self.with_mode(self.mode.toggled())
    }

    /// This state in entry mode `entry_mode`.
    pub fn with_entry_mode(&self, entry_mode: DatePickerEntryMode) -> Self {
        Self {
            entry_mode,
            ..self.clone()
        }
    }

    /// This state with the entry mode toggled (calendar ⇄ input), **preserving
    /// the selection and its formatted text** — the reference's
    /// `_handleEntryModeToggle`, whose `_formKey.currentState?.save()` on the
    /// way out of input mode exists for exactly this round trip.
    ///
    /// A locked (`*Only`) entry mode reports itself unchanged.
    pub fn toggled_entry_mode(&self) -> Self {
        self.with_entry_mode(self.entry_mode.toggled())
    }

    /// This state with the input field's draft text replaced. The selection is
    /// **not** touched — a widget parses the text itself and calls
    /// [`Self::with_selected`] separately once it resolves to a selectable
    /// date, mirroring the reference's split between `onChanged`
    /// (`field.didChange`) and `onDateSubmitted`/`onDateSaved`.
    pub fn with_input_text(&self, text: impl Into<String>) -> Self {
        Self {
            input_text: text.into(),
            ..self.clone()
        }
    }

    /// The date a tap on year cell `year` would select, before any
    /// selectability predicate: the current selection's month/day carried onto
    /// `year` (shortened where that month is shorter) and clamped into
    /// `first..=last` — the reference's `_buildYearCell` `candidate`.
    pub fn year_candidate(
        &self,
        year: i32,
        first: MaterialDate,
        last: MaterialDate,
    ) -> MaterialDate {
        let base = self.selected.unwrap_or(self.displayed_month);
        MaterialDate::new(year, base.month(), base.day()).clamp_to(first, last)
    }

    /// This state after a tap on year cell `year`: back to the day sub-view,
    /// showing (and selecting) [`Self::year_candidate`] — the reference's
    /// `_handleYearChanged`.
    ///
    /// A caller with a selectability predicate checks
    /// [`Self::year_candidate`] first and falls back to
    /// `self.with_displayed_month(..).with_mode(Day)` when the candidate is
    /// refused, which is what the reference's `if (isSelectable(clamped))`
    /// guard does.
    pub fn with_year(&self, year: i32, first: MaterialDate, last: MaterialDate) -> Self {
        let candidate = self.year_candidate(year, first, last);
        self.with_selected(candidate).with_mode(DatePickerMode::Day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> MaterialDate {
        MaterialDate::new(year, month, day)
    }

    const FIRST: fn() -> MaterialDate = || MaterialDate::new(2020, 1, 1);
    const LAST: fn() -> MaterialDate = || MaterialDate::new(2030, 12, 31);

    // ---- constants -------------------------------------------------------

    #[test]
    fn the_ported_metrics_match_the_reference_constants() {
        // m3e_date_picker_constants.dart / m3e_date_picker_theme.dart.
        assert_eq!(DAYS_PER_WEEK, 7);
        assert_eq!(SUB_HEADER_HEIGHT, 52.0);
        assert_eq!(WEEKDAY_ROW_HEIGHT, 24.0);
        assert_eq!(DAY_ROW_HEIGHT, 48.0);
        assert_eq!(DAY_GRID_TOP_PADDING, 4.0);
        assert_eq!(MAX_DAY_PICKER_ROW_COUNT, 6);
        assert_eq!(MAX_DAY_PICKER_HEIGHT, 336.0);
        assert_eq!(MONTH_NAV_BUTTONS_WIDTH, 108.0);
        assert_eq!(YEAR_COLUMN_COUNT, 3);
        assert_eq!(YEAR_GRID_PADDING, 16.0);
        assert_eq!(YEAR_ROW_HEIGHT, 52.0);
        assert_eq!(YEAR_ROW_SPACING, 8.0);
        assert_eq!(CALENDAR_WIDTH, 328.0);
        assert_eq!(CALENDAR_PADDING, 12.0);
        assert_eq!(DAY_SIZE, 40.0);
        assert_eq!(ARROW_ICON_SIZE, 24.0);
        assert_eq!(ARROW_PADDING, 8.0);
        assert_eq!(HEADER_PORTRAIT_HEIGHT, 120.0);
        assert_eq!(ACTIONS_MIN_HEIGHT, 52.0);
        assert_eq!(MONTH_SCROLL_DURATION, Duration::from_millis(200));
        assert_eq!(DIALOG_PORTRAIT_CALENDAR_WIDTH, 360.0);
        assert_eq!(DIALOG_PORTRAIT_INPUT_WIDTH, 328.0);
        assert_eq!(DISABLED_DAY_OPACITY, 0.38);
        assert_eq!(RANGE_HIGHLIGHT_ALPHA, 0.12);
    }

    // ---- enums -----------------------------------------------------------

    #[test]
    fn the_calendar_sub_view_toggles_both_ways() {
        assert_eq!(DatePickerMode::default(), DatePickerMode::Day);
        assert_eq!(DatePickerMode::Day.toggled(), DatePickerMode::Year);
        assert_eq!(DatePickerMode::Year.toggled(), DatePickerMode::Day);
    }

    #[test]
    fn only_the_unlocked_entry_modes_toggle() {
        assert_eq!(
            DatePickerEntryMode::default(),
            DatePickerEntryMode::Calendar
        );
        assert_eq!(
            DatePickerEntryMode::Calendar.toggled(),
            DatePickerEntryMode::Input
        );
        assert_eq!(
            DatePickerEntryMode::Input.toggled(),
            DatePickerEntryMode::Calendar
        );
        // A `*Only` mode reports itself — the reference's `break`.
        assert_eq!(
            DatePickerEntryMode::CalendarOnly.toggled(),
            DatePickerEntryMode::CalendarOnly
        );
        assert_eq!(
            DatePickerEntryMode::InputOnly.toggled(),
            DatePickerEntryMode::InputOnly
        );

        assert!(!DatePickerEntryMode::Calendar.is_input());
        assert!(DatePickerEntryMode::Input.is_input());
        assert!(!DatePickerEntryMode::CalendarOnly.is_input());
        assert!(DatePickerEntryMode::InputOnly.is_input());

        assert!(DatePickerEntryMode::Calendar.is_toggleable());
        assert!(DatePickerEntryMode::Input.is_toggleable());
        assert!(!DatePickerEntryMode::CalendarOnly.is_toggleable());
        assert!(!DatePickerEntryMode::InputOnly.is_toggleable());
    }

    // ---- strings ---------------------------------------------------------

    #[test]
    fn the_weekday_header_rotates_with_the_first_day_of_week() {
        let sunday = DatePickerStrings::ENGLISH;
        let columns: Vec<&str> = (0..7).map(|c| sunday.weekday_initial(c)).collect();
        assert_eq!(columns, ["S", "M", "T", "W", "T", "F", "S"]);

        let monday = DatePickerStrings {
            first_day_of_week: 1,
            ..DatePickerStrings::ENGLISH
        };
        let columns: Vec<&str> = (0..7).map(|c| monday.weekday_initial(c)).collect();
        assert_eq!(columns, ["M", "T", "W", "T", "F", "S", "S"]);
    }

    #[test]
    fn the_label_formats_match_the_en_us_patterns() {
        let s = DatePickerStrings::ENGLISH;
        assert_eq!(s.month_year(d(2026, 3, 1)), "March 2026");
        assert_eq!(s.month_year(d(2026, 12, 31)), "December 2026");
        assert_eq!(s.year(2026), "2026");
        // 2026-08-20 is a Thursday.
        assert_eq!(s.medium_date(d(2026, 8, 20)), "Thu, Aug 20");
        // 2026-01-01 is a Thursday too.
        assert_eq!(s.medium_date(d(2026, 1, 1)), "Thu, Jan 1");
    }

    #[test]
    fn an_override_keeps_every_untranslated_field() {
        let french = DatePickerStrings {
            months: [
                "janvier",
                "février",
                "mars",
                "avril",
                "mai",
                "juin",
                "juillet",
                "août",
                "septembre",
                "octobre",
                "novembre",
                "décembre",
            ],
            first_day_of_week: 1,
            cancel_label: "Annuler",
            ..DatePickerStrings::ENGLISH
        };
        assert_eq!(french.month_year(d(2026, 3, 1)), "mars 2026");
        assert_eq!(french.cancel_label, "Annuler");
        assert_eq!(
            french.confirm_label,
            DatePickerStrings::ENGLISH.confirm_label
        );
        assert_eq!(french.weekday_initial(0), "M");
    }

    // ---- state transitions ------------------------------------------------

    #[test]
    fn a_fresh_state_shows_the_selected_month_or_today() {
        let today = d(2026, 8, 20);
        let empty = DatePickerState::new(None, today);
        assert_eq!(empty.selected, None);
        assert_eq!(empty.displayed_month, d(2026, 8, 1));
        assert_eq!(empty.mode, DatePickerMode::Day);
        assert_eq!(empty.entry_mode, DatePickerEntryMode::Calendar);
        assert_eq!(empty.input_text, "");

        let seeded = DatePickerState::new(Some(d(2020, 2, 29)), today);
        assert_eq!(seeded.selected, Some(d(2020, 2, 29)));
        assert_eq!(seeded.displayed_month, d(2020, 2, 1));
        assert_eq!(seeded.input_text, "02/29/2020");
    }

    #[test]
    fn selecting_records_the_date_moves_the_month_and_refreshes_the_text() {
        let state = DatePickerState::new(None, d(2026, 8, 20));
        let next = state.with_selected(d(2027, 3, 5));
        assert_eq!(next.selected, Some(d(2027, 3, 5)));
        assert_eq!(next.displayed_month, d(2027, 3, 1));
        assert_eq!(next.input_text, "03/05/2027");
        // The original is untouched — every transition is pure.
        assert_eq!(state.selected, None);
    }

    #[test]
    fn month_paging_clamps_at_both_bounds() {
        let state = DatePickerState::new(Some(d(2020, 1, 15)), d(2026, 8, 20));
        assert_eq!(state.displayed_month, d(2020, 1, 1));

        // Already at the first month: stepping back is a no-op.
        assert!(!state.can_step_month(-1, FIRST(), LAST()));
        assert_eq!(
            state.stepped_month(-1, FIRST(), LAST()).displayed_month,
            d(2020, 1, 1)
        );

        // Forward moves one month at a time, and the selection stays put.
        assert!(state.can_step_month(1, FIRST(), LAST()));
        let next = state.stepped_month(1, FIRST(), LAST());
        assert_eq!(next.displayed_month, d(2020, 2, 1));
        assert_eq!(next.selected, state.selected);

        // A jump past the last month clamps onto it.
        let end = state.stepped_month(10_000, FIRST(), LAST());
        assert_eq!(end.displayed_month, d(2030, 12, 1));
        assert!(!end.can_step_month(1, FIRST(), LAST()));
    }

    #[test]
    fn the_sub_view_toggle_round_trips() {
        let state = DatePickerState::new(Some(d(2026, 8, 20)), d(2026, 8, 20));
        let year = state.toggled_mode();
        assert_eq!(year.mode, DatePickerMode::Year);
        assert_eq!(year.toggled_mode().mode, DatePickerMode::Day);
        assert_eq!(year.selected, state.selected, "the toggle keeps the value");
    }

    #[test]
    fn the_entry_mode_toggle_round_trips_the_value_and_its_text() {
        let state = DatePickerState::new(Some(d(2026, 8, 20)), d(2026, 8, 20));
        assert_eq!(state.input_text, "08/20/2026");

        let input = state.toggled_entry_mode();
        assert_eq!(input.entry_mode, DatePickerEntryMode::Input);
        assert_eq!(input.selected, state.selected);
        assert_eq!(input.input_text, state.input_text);

        let back = input.toggled_entry_mode();
        assert_eq!(back.entry_mode, DatePickerEntryMode::Calendar);
        assert_eq!(back.selected, state.selected);
        assert_eq!(back.input_text, state.input_text);
        assert_eq!(back, state, "a full round trip is the identity");
    }

    #[test]
    fn a_locked_entry_mode_cannot_be_toggled_out_of() {
        let locked = DatePickerState::new(None, d(2026, 8, 20))
            .with_entry_mode(DatePickerEntryMode::InputOnly);
        assert_eq!(
            locked.toggled_entry_mode().entry_mode,
            DatePickerEntryMode::InputOnly
        );
    }

    #[test]
    fn typing_replaces_only_the_draft_text() {
        let state = DatePickerState::new(Some(d(2026, 8, 20)), d(2026, 8, 20));
        let typing = state.with_input_text("08/2");
        assert_eq!(typing.input_text, "08/2");
        assert_eq!(typing.selected, state.selected, "typing never picks a date");
        assert_eq!(typing.displayed_month, state.displayed_month);
    }

    #[test]
    fn a_year_cell_carries_the_selection_forward_and_returns_to_the_day_grid() {
        let state = DatePickerState::new(Some(d(2026, 3, 31)), d(2026, 8, 20))
            .with_mode(DatePickerMode::Year);
        let next = state.with_year(2028, FIRST(), LAST());
        assert_eq!(next.mode, DatePickerMode::Day);
        assert_eq!(next.selected, Some(d(2028, 3, 31)));
        assert_eq!(next.displayed_month, d(2028, 3, 1));
        assert_eq!(next.input_text, "03/31/2028");
    }

    #[test]
    fn a_year_candidate_shortens_a_too_long_day_and_clamps_to_the_bounds() {
        // 29 February only exists in a leap year.
        let leap = DatePickerState::new(Some(d(2024, 2, 29)), d(2026, 8, 20));
        assert_eq!(leap.year_candidate(2025, FIRST(), LAST()), d(2025, 2, 28));
        assert_eq!(leap.year_candidate(2028, FIRST(), LAST()), d(2028, 2, 29));

        // A year cell outside the bounds clamps onto the nearest bound.
        let narrow_first = d(2026, 6, 1);
        let narrow_last = d(2026, 6, 30);
        let state = DatePickerState::new(Some(d(2026, 6, 15)), d(2026, 6, 15));
        assert_eq!(
            state.year_candidate(2020, narrow_first, narrow_last),
            narrow_first
        );
        assert_eq!(
            state.year_candidate(2030, narrow_first, narrow_last),
            narrow_last
        );
    }

    #[test]
    fn a_year_candidate_falls_back_to_the_displayed_month_with_no_selection() {
        let state = DatePickerState::new(None, d(2026, 8, 20));
        assert_eq!(state.displayed_month, d(2026, 8, 1));
        assert_eq!(state.year_candidate(2029, FIRST(), LAST()), d(2029, 8, 1));
    }
}
