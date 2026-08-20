// Ported from `material_3_expressive` v1.0.8's date-picker date helpers (MIT,
// © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/date_pickers/utils/m3e_date_picker_utils.dart`,
// `models/m3e_date_picker_models.dart`, `models/m3e_calendar_labels.dart`,
// retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decision: the reference reaches Dart's built-in `DateTime` for every
// calendar computation — `DateTime(year, month + 1, 0).day` for a month's
// length, `.weekday` for the grid offset, `isBefore`/`isAfter` for bounds. This
// crate's production dependencies are `frust` + `kurbo`/`peniko` only (no
// `chrono`, no `time` — see `docs/PLUGINS_ARCHITECTURE.md`'s Design-System
// Plugins), so the civil-calendar arithmetic is implemented here directly, over
// a proleptic-Gregorian day count.

//! The date-picker family's own civil-calendar value type and arithmetic.
//!
//! [`MaterialDate`] is a whole-day, time-zone-free, proleptic-Gregorian date:
//! exactly the `DateTime(y, m, d)` shape the reference normalizes every input
//! to through its `M3EDatePickerUtils.dateOnly`, with the time component gone
//! rather than pinned to midnight.
//!
//! # No date library, by charter
//!
//! Everything a calendar grid needs is bounded and closed-form: how long a
//! month is, whether a year leaps, and which weekday a month starts on. All
//! three are derived here from one primitive — [`MaterialDate::epoch_day`],
//! Howard Hinnant's `days_from_civil` (public domain, `chrono`-Compatible Howard
//! Hinnant date algorithms, <https://howardhinnant.github.io/date_algorithms.html>)
//! — which additionally gives day-difference and ordering for free.
//!
//! # Clamping, not failing
//!
//! [`MaterialDate::new`] never fails: the year is clamped to
//! [`MIN_YEAR`]`..=`[`MAX_YEAR`], the month to `1..=12`, and the day to that
//! month's own length. A calendar widget resolving a month page or a year cell
//! has no useful error path — an out-of-range component means "the nearest real
//! date", the same way [`MaterialDate::add_months`] shortens 31 January by a
//! month into 28/29 February (the reference's own `normalizeSelectedDay`).
//! [`MaterialDate::parse_compact`], by contrast, *does* reject — a user typing
//! `13/40/2026` must see the field's format error, not a silently clamped date.

/// The earliest year [`MaterialDate`] represents. Year 1 rather than year 0:
/// the proleptic Gregorian calendar has no year 0, and a picker that could
/// address one would format it as a date no locale can parse back.
pub const MIN_YEAR: i32 = 1;

/// The latest year [`MaterialDate`] represents — the four-digit ceiling
/// [`MaterialDate::format_compact`]'s `yyyy` field can round-trip.
pub const MAX_YEAR: i32 = 9999;

/// Whether `year` is a leap year in the proleptic Gregorian calendar: divisible
/// by 4, except centuries, except every fourth century.
///
/// ```
/// use frust_material::date_picker::is_leap_year;
///
/// assert!(is_leap_year(2024));
/// assert!(is_leap_year(2000));
/// assert!(!is_leap_year(1900));
/// assert!(!is_leap_year(2023));
/// ```
pub const fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// How many days `month` (`1..=12`, clamped) has in `year` — the reference's
/// `M3EDatePickerUtils.daysInMonth`.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month.clamp(1, 12) {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // February.
        _ => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
    }
}

/// A whole calendar day: year, month (`1..=12`), day (`1..=`[`days_in_month`]).
///
/// Named `MaterialDate` rather than a bare `Date` because this crate flat
/// re-exports every widget/spec type at its root (`frust_material::MaterialDate`),
/// where a bare `Date` would collide with any other catalog or app type of the
/// same name — the same reason [`crate::MaterialTokens`]/[`crate::MaterialMotion`]
/// carry the prefix.
///
/// Ordering is chronological ([`Ord`] over `(year, month, day)`, which is
/// equivalent to ordering by [`Self::epoch_day`] because every field is
/// normalized on construction), so `first_date <= day && day <= last_date` is
/// the bounds test the whole family uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialDate {
    year: i32,
    month: u32,
    day: u32,
}

impl MaterialDate {
    /// The nearest real date to `(year, month, day)`: the year clamped to
    /// [`MIN_YEAR`]`..=`[`MAX_YEAR`], the month to `1..=12`, the day to that
    /// month's own length. See the [module docs](self)' clamping note.
    ///
    /// ```
    /// use frust_material::MaterialDate;
    ///
    /// // 31 February resolves to the 29th in a leap year, the 28th otherwise.
    /// assert_eq!(MaterialDate::new(2024, 2, 31), MaterialDate::new(2024, 2, 29));
    /// assert_eq!(MaterialDate::new(2023, 2, 31), MaterialDate::new(2023, 2, 28));
    /// ```
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        let year = year.clamp(MIN_YEAR, MAX_YEAR);
        let month = month.clamp(1, 12);
        let day = day.clamp(1, days_in_month(year, month));
        Self { year, month, day }
    }

    /// This date's year.
    pub fn year(self) -> i32 {
        self.year
    }

    /// This date's month, `1..=12`.
    pub fn month(self) -> u32 {
        self.month
    }

    /// This date's day of the month, `1..=`[`days_in_month`].
    pub fn day(self) -> u32 {
        self.day
    }

    /// The first of this date's own month — the reference's
    /// `M3EDatePickerUtils.getMonth(date.year, date.month)`, which is how every
    /// "displayed month" value in this family is normalized.
    pub fn first_of_month(self) -> Self {
        Self {
            year: self.year,
            month: self.month,
            day: 1,
        }
    }

    /// Days since 1970-01-01 (negative before it) — Howard Hinnant's
    /// `days_from_civil`. The one primitive [`Self::weekday_index`] and
    /// [`Self::days_until`] are derived from.
    ///
    /// ```
    /// use frust_material::MaterialDate;
    ///
    /// assert_eq!(MaterialDate::new(1970, 1, 1).epoch_day(), 0);
    /// assert_eq!(MaterialDate::new(1969, 12, 31).epoch_day(), -1);
    /// ```
    pub fn epoch_day(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        // Year of era, 0..=399.
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let d = i64::from(self.day);
        // Day of year counted from 1 March, 0..=365.
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        // Day of era, 0..=146096.
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Signed day count from `self` to `other` (`other - self`).
    pub fn days_until(self, other: Self) -> i64 {
        other.epoch_day() - self.epoch_day()
    }

    /// This date's weekday as a **Sunday-first** index: `0` Sunday through `6`
    /// Saturday.
    ///
    /// The reference reads Dart's `DateTime.weekday` (Monday-first, `1..=7`)
    /// and folds it with `% 7` (`M3ECalendarLabels.firstWeekday`); this returns
    /// the folded value directly, since every consumer here — the grid's
    /// leading-blank count and the weekday header row — wants the Sunday-first
    /// one.
    ///
    /// ```
    /// use frust_material::MaterialDate;
    ///
    /// // 2000-01-01 was a Saturday; 2026-08-20 a Thursday.
    /// assert_eq!(MaterialDate::new(2000, 1, 1).weekday_index(), 6);
    /// assert_eq!(MaterialDate::new(2026, 8, 20).weekday_index(), 4);
    /// ```
    pub fn weekday_index(self) -> u32 {
        (self.epoch_day() + 4).rem_euclid(7) as u32
    }

    /// How many days this date's own month has.
    pub fn days_in_this_month(self) -> u32 {
        days_in_month(self.year, self.month)
    }

    /// This date shifted by `delta` whole months, keeping the day of the month
    /// where the target month is long enough and shortening it where it is not
    /// — the reference's `addMonthsToMonthDate` + `normalizeSelectedDay` pair.
    ///
    /// ```
    /// use frust_material::MaterialDate;
    ///
    /// let jan31 = MaterialDate::new(2026, 1, 31);
    /// assert_eq!(jan31.add_months(1), MaterialDate::new(2026, 2, 28));
    /// assert_eq!(jan31.add_months(-1), MaterialDate::new(2025, 12, 31));
    /// ```
    pub fn add_months(self, delta: i32) -> Self {
        let total = i64::from(self.year) * 12 + i64::from(self.month) - 1 + i64::from(delta);
        let year = total.div_euclid(12);
        let month = total.rem_euclid(12) as u32 + 1;
        // `new` clamps the year back into range, so a runaway `delta` saturates
        // at the calendar's ends rather than wrapping.
        let year = year.clamp(i64::from(MIN_YEAR), i64::from(MAX_YEAR)) as i32;
        Self::new(year, month, self.day)
    }

    /// Whole months from `self`'s month to `other`'s — the reference's
    /// `monthDelta`. Day-of-month is ignored on both sides.
    pub fn month_delta(self, other: Self) -> i32 {
        (other.year - self.year) * 12 + other.month as i32 - self.month as i32
    }

    /// `self` clamped into `first..=last` (the reference's `clampDate`). A
    /// reversed pair (`last < first`) collapses onto `first`, so a caller that
    /// hands the bounds in the wrong order gets a defined date rather than a
    /// panic.
    pub fn clamp_to(self, first: Self, last: Self) -> Self {
        if self < first {
            return first;
        }
        if self > last {
            return if last < first { first } else { last };
        }
        self
    }

    /// Whether `self` lies within `first..=last`, inclusive — the reference's
    /// `isSelectable` *bounds* half (its optional predicate is applied by the
    /// widget, which owns the caller-supplied closure).
    pub fn is_within(self, first: Self, last: Self) -> bool {
        self >= first && self <= last
    }

    /// This date in the compact, parseable `mm/dd/yyyy` form — Flutter's
    /// `MaterialLocalizations.formatCompactDate` for `en_US`, which is the
    /// format the reference's input mode both renders and parses.
    ///
    /// See [`crate::DatePickerStrings`] for why this port ships one fixed
    /// English/US format rather than a locale-resolved one.
    ///
    /// ```
    /// use frust_material::MaterialDate;
    ///
    /// assert_eq!(MaterialDate::new(2026, 8, 20).format_compact(), "08/20/2026");
    /// ```
    pub fn format_compact(self) -> String {
        format!("{:02}/{:02}/{:04}", self.month, self.day, self.year)
    }

    /// Parse the compact `mm/dd/yyyy` form back into a date, or `None` if the
    /// text is not exactly that — Flutter's `parseCompactDate` for `en_US`,
    /// which is what drives the input mode's *format* error.
    ///
    /// Rejects (rather than clamping — see the [module docs](self)): a part
    /// count other than three, any non-ASCII-digit character, an empty part, a
    /// month outside `1..=12`, a day outside the month's own length, and a year
    /// outside [`MIN_YEAR`]`..=`[`MAX_YEAR`]. Surrounding whitespace is trimmed
    /// first, so a paste with a trailing newline still parses.
    ///
    /// ```
    /// use frust_material::MaterialDate;
    ///
    /// assert_eq!(MaterialDate::parse_compact("08/20/2026"), Some(MaterialDate::new(2026, 8, 20)));
    /// assert_eq!(MaterialDate::parse_compact("8/2/2026"), Some(MaterialDate::new(2026, 8, 2)));
    /// assert_eq!(MaterialDate::parse_compact("02/30/2026"), None);
    /// assert_eq!(MaterialDate::parse_compact("2026-08-20"), None);
    /// ```
    pub fn parse_compact(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('/');
        let month = parse_field(parts.next()?, 2)?;
        let day = parse_field(parts.next()?, 2)?;
        let year = parse_field(parts.next()?, 4)?;
        if parts.next().is_some() {
            return None;
        }
        let year = i32::try_from(year).ok()?;
        if !(MIN_YEAR..=MAX_YEAR).contains(&year) || !(1..=12).contains(&month) {
            return None;
        }
        if day < 1 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }
}

/// One `/`-separated field of a compact date: `1..=max_digits` ASCII digits and
/// nothing else.
fn parse_field(text: &str, max_digits: usize) -> Option<u32> {
    if text.is_empty() || text.len() > max_digits || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok()
}

/// A selected date range: a `start`, and an `end` that is `None` while the user
/// is mid-selection — the reference's `M3EDateRange`.
///
/// # Scope: the model ships, the range *dialog* does not
///
/// The reference ships a full range picker
/// (`M3EDateRangePickerDialog`/`M3ECalendarDateRangePicker`), presented as a
/// vertically scrolling list of *every* month between `firstDate` and
/// `lastDate`. This port carries the range **model** (this type, plus
/// [`Self::extend`]'s selection rule) and the day cell's range visuals
/// ([`crate::CalendarDatePicker::range`]), so an app can drive range selection
/// against the single-month calendar; the scrolling multi-month dialog itself
/// is a deliberate v1 cut, for the same reason [`crate::dialog`]'s selection
/// list has no scroll physics — see [`mod@crate::date_picker`]'s scope section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateRange {
    /// The first day of the range.
    pub start: MaterialDate,
    /// The last day of the range, or `None` while only a start has been picked.
    pub end: Option<MaterialDate>,
}

impl DateRange {
    /// A range with only its `start` picked.
    pub fn new(start: MaterialDate) -> Self {
        Self { start, end: None }
    }

    /// A closed `start..=end` range. A reversed pair is swapped rather than
    /// stored inverted, so [`Self::contains`] can never be vacuously false.
    pub fn closed(start: MaterialDate, end: MaterialDate) -> Self {
        if end < start {
            Self {
                start: end,
                end: Some(start),
            }
        } else {
            Self {
                start,
                end: Some(end),
            }
        }
    }

    /// Whether both ends are picked — the reference's `M3EDateRange.isComplete`.
    pub fn is_complete(self) -> bool {
        self.end.is_some()
    }

    /// Whether `day` lies inside the range, inclusive of both ends. An
    /// unfinished range contains only its own start.
    pub fn contains(self, day: MaterialDate) -> bool {
        match self.end {
            Some(end) => day >= self.start && day <= end,
            None => day == self.start,
        }
    }

    /// The range a tap on `date` produces, given `current` — the reference's
    /// `_M3ECalendarDateRangePickerState._updateSelection`: a tap **closes** an
    /// open range when it lands on or after its start, and otherwise **restarts**
    /// a fresh open range at the tapped day.
    ///
    /// ```
    /// use frust_material::{DateRange, MaterialDate};
    ///
    /// let d = |day| MaterialDate::new(2026, 8, day);
    /// let opened = DateRange::extend(None, d(3));
    /// assert_eq!(opened, DateRange::new(d(3)));
    ///
    /// // A later day closes it...
    /// assert_eq!(DateRange::extend(Some(opened), d(9)).end, Some(d(9)));
    /// // ...an earlier one restarts it.
    /// assert_eq!(DateRange::extend(Some(opened), d(1)), DateRange::new(d(1)));
    /// // A tap on a *closed* range always restarts.
    /// let closed = DateRange::closed(d(3), d(9));
    /// assert_eq!(DateRange::extend(Some(closed), d(5)), DateRange::new(d(5)));
    /// ```
    pub fn extend(current: Option<Self>, date: MaterialDate) -> Self {
        match current {
            Some(range) if range.end.is_none() && date >= range.start => Self {
                start: range.start,
                end: Some(date),
            },
            _ => Self::new(date),
        }
    }
}

#[cfg(test)]
impl MaterialDate {
    /// Test-only inverse of [`Self::epoch_day`] (Hinnant's `civil_from_days`),
    /// used to walk a weekday cycle across month/year boundaries without
    /// hand-writing every date.
    fn from_epoch_day_for_test(z: i64) -> Self {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Self::new((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- leap years ----------------------------------------------------

    #[test]
    fn the_leap_rule_covers_all_three_century_cases() {
        // Divisible by 400 leaps; by 100 (not 400) does not; by 4 does.
        assert!(is_leap_year(2000));
        assert!(is_leap_year(1600));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2100));
        assert!(is_leap_year(2024));
        assert!(is_leap_year(2028));
        assert!(!is_leap_year(2023));
        assert!(!is_leap_year(2025));
    }

    #[test]
    fn february_follows_the_leap_rule_and_every_other_month_is_fixed() {
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);

        let lengths = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for (i, expected) in lengths.into_iter().enumerate() {
            let month = i as u32 + 1;
            assert_eq!(days_in_month(2023, month), expected, "month {month}");
        }
        // A leap year differs in exactly one month.
        let leap_total: u32 = (1..=12).map(|m| days_in_month(2024, m)).sum();
        let common_total: u32 = (1..=12).map(|m| days_in_month(2023, m)).sum();
        assert_eq!(leap_total, 366);
        assert_eq!(common_total, 365);
    }

    #[test]
    fn an_out_of_range_month_clamps_rather_than_panicking() {
        assert_eq!(days_in_month(2026, 0), days_in_month(2026, 1));
        assert_eq!(days_in_month(2026, 13), days_in_month(2026, 12));
    }

    // ---- weekday -------------------------------------------------------

    #[test]
    fn weekdays_are_pinned_against_known_dates() {
        // Sunday-first indices: 0 Sun, 1 Mon, 2 Tue, 3 Wed, 4 Thu, 5 Fri, 6 Sat.
        let cases = [
            (MaterialDate::new(1970, 1, 1), 4),   // Thursday (epoch)
            (MaterialDate::new(2000, 1, 1), 6),   // Saturday
            (MaterialDate::new(1900, 1, 1), 1),   // Monday
            (MaterialDate::new(2024, 2, 29), 4),  // Thursday (leap day)
            (MaterialDate::new(2026, 8, 20), 4),  // Thursday
            (MaterialDate::new(2026, 3, 1), 0),   // Sunday
            (MaterialDate::new(2025, 12, 25), 4), // Thursday
            (MaterialDate::new(1969, 7, 20), 0),  // Sunday (Apollo 11)
        ];
        for (date, expected) in cases {
            assert_eq!(date.weekday_index(), expected, "{date:?}");
        }
    }

    #[test]
    fn the_weekday_cycle_advances_by_one_per_day_across_a_month_boundary() {
        let mut date = MaterialDate::new(2024, 2, 26);
        let mut expected = date.weekday_index();
        for _ in 0..10 {
            assert_eq!(date.weekday_index(), expected);
            date = MaterialDate::from_epoch_day_for_test(date.epoch_day() + 1);
            expected = (expected + 1) % 7;
        }
        // Ten days on from 26 Feb 2024 (a leap year) is 7 March.
        assert_eq!(date, MaterialDate::new(2024, 3, 7));
    }

    // ---- epoch day -----------------------------------------------------

    #[test]
    fn epoch_days_are_pinned_and_monotonic() {
        assert_eq!(MaterialDate::new(1970, 1, 1).epoch_day(), 0);
        assert_eq!(MaterialDate::new(1970, 1, 2).epoch_day(), 1);
        assert_eq!(MaterialDate::new(1969, 12, 31).epoch_day(), -1);
        assert_eq!(MaterialDate::new(2000, 1, 1).epoch_day(), 10_957);
        assert_eq!(MaterialDate::new(2026, 1, 1).epoch_day(), 20_454);
        assert_eq!(
            MaterialDate::new(2023, 1, 1).days_until(MaterialDate::new(2024, 1, 1)),
            365
        );
        assert_eq!(
            MaterialDate::new(2024, 1, 1).days_until(MaterialDate::new(2025, 1, 1)),
            366,
            "2024 is a leap year"
        );
    }

    #[test]
    fn ordering_is_chronological() {
        let a = MaterialDate::new(2026, 1, 31);
        let b = MaterialDate::new(2026, 2, 1);
        let c = MaterialDate::new(2027, 1, 1);
        assert!(a < b && b < c);
        assert!(a.is_within(a, c));
        assert!(!c.is_within(a, b));
        // Tuple ordering and epoch-day ordering agree on normalized values.
        assert!(a.epoch_day() < b.epoch_day());
    }

    // ---- construction / clamping ---------------------------------------

    #[test]
    fn construction_clamps_every_component() {
        assert_eq!(
            MaterialDate::new(2024, 2, 31),
            MaterialDate::new(2024, 2, 29)
        );
        assert_eq!(
            MaterialDate::new(2023, 2, 31),
            MaterialDate::new(2023, 2, 28)
        );
        assert_eq!(MaterialDate::new(2026, 0, 5).month(), 1);
        assert_eq!(MaterialDate::new(2026, 99, 5).month(), 12);
        assert_eq!(MaterialDate::new(2026, 4, 0).day(), 1);
        assert_eq!(MaterialDate::new(-5, 1, 1).year(), MIN_YEAR);
        assert_eq!(MaterialDate::new(50_000, 1, 1).year(), MAX_YEAR);
    }

    #[test]
    fn first_of_month_keeps_the_month_and_drops_the_day() {
        let d = MaterialDate::new(2026, 8, 20);
        assert_eq!(d.first_of_month(), MaterialDate::new(2026, 8, 1));
        assert_eq!(d.first_of_month().first_of_month(), d.first_of_month());
    }

    // ---- month arithmetic ----------------------------------------------

    #[test]
    fn add_months_wraps_the_year_and_shortens_a_too_long_day() {
        let jan31 = MaterialDate::new(2026, 1, 31);
        assert_eq!(jan31.add_months(1), MaterialDate::new(2026, 2, 28));
        assert_eq!(jan31.add_months(11), MaterialDate::new(2026, 12, 31));
        assert_eq!(jan31.add_months(12), MaterialDate::new(2027, 1, 31));
        assert_eq!(jan31.add_months(-1), MaterialDate::new(2025, 12, 31));
        assert_eq!(jan31.add_months(-13), MaterialDate::new(2024, 12, 31));
        assert_eq!(jan31.add_months(0), jan31);
        // A leap February keeps the 29th.
        assert_eq!(
            MaterialDate::new(2024, 1, 31).add_months(1),
            MaterialDate::new(2024, 2, 29)
        );
    }

    #[test]
    fn add_months_saturates_at_the_calendar_ends() {
        let last = MaterialDate::new(MAX_YEAR, 12, 31);
        assert_eq!(last.add_months(1).year(), MAX_YEAR);
        let first = MaterialDate::new(MIN_YEAR, 1, 1);
        assert_eq!(first.add_months(-1).year(), MIN_YEAR);
        // Even an absurd delta stays in range rather than wrapping.
        assert_eq!(last.add_months(i32::MAX).year(), MAX_YEAR);
        assert_eq!(first.add_months(i32::MIN).year(), MIN_YEAR);
    }

    #[test]
    fn month_delta_counts_whole_months_and_ignores_the_day() {
        let a = MaterialDate::new(2026, 1, 31);
        let b = MaterialDate::new(2026, 3, 1);
        assert_eq!(a.month_delta(b), 2);
        assert_eq!(b.month_delta(a), -2);
        assert_eq!(a.month_delta(a), 0);
        assert_eq!(
            MaterialDate::new(2020, 1, 1).month_delta(MaterialDate::new(2030, 1, 1)),
            120
        );
    }

    #[test]
    fn clamp_to_pins_both_ends_and_survives_a_reversed_pair() {
        let first = MaterialDate::new(2026, 1, 1);
        let last = MaterialDate::new(2026, 12, 31);
        assert_eq!(MaterialDate::new(2025, 6, 1).clamp_to(first, last), first);
        assert_eq!(MaterialDate::new(2027, 6, 1).clamp_to(first, last), last);
        let inside = MaterialDate::new(2026, 6, 1);
        assert_eq!(inside.clamp_to(first, last), inside);
        // Reversed bounds collapse onto `first` rather than producing a value
        // outside both.
        assert_eq!(MaterialDate::new(2027, 6, 1).clamp_to(last, first), last);
    }

    // ---- compact format / parse ----------------------------------------

    #[test]
    fn compact_formatting_zero_pads_every_field() {
        assert_eq!(
            MaterialDate::new(2026, 8, 20).format_compact(),
            "08/20/2026"
        );
        assert_eq!(MaterialDate::new(2026, 1, 1).format_compact(), "01/01/2026");
        assert_eq!(
            MaterialDate::new(999, 12, 31).format_compact(),
            "12/31/0999"
        );
    }

    #[test]
    fn compact_parsing_round_trips_every_formatted_date() {
        for year in [1900, 1970, 2000, 2024, 2026] {
            for month in 1..=12 {
                for day in [1, 15, days_in_month(year, month)] {
                    let d = MaterialDate::new(year, month, day);
                    assert_eq!(
                        MaterialDate::parse_compact(&d.format_compact()),
                        Some(d),
                        "{d:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn compact_parsing_accepts_unpadded_fields_and_trims_whitespace() {
        let expected = Some(MaterialDate::new(2026, 8, 2));
        assert_eq!(MaterialDate::parse_compact("8/2/2026"), expected);
        assert_eq!(MaterialDate::parse_compact("  08/02/2026 \n"), expected);
    }

    #[test]
    fn compact_parsing_rejects_every_malformed_shape() {
        for bad in [
            "",
            "   ",
            "2026-08-20",
            "08/20",
            "08/20/2026/1",
            "08//2026",
            "/20/2026",
            "08/20/",
            "aa/20/2026",
            "08/2o/2026",
            "+8/20/2026",
            "08/20/ 2026",
            "13/01/2026",  // month out of range
            "00/01/2026",  // month zero
            "02/30/2026",  // day past February
            "02/29/2023",  // not a leap year
            "04/31/2026",  // day past a 30-day month
            "08/20/00000", // five-digit year
            "08/20/0000",  // year below MIN_YEAR
        ] {
            assert_eq!(MaterialDate::parse_compact(bad), None, "accepted {bad:?}");
        }
        // 29 February *is* accepted in a leap year.
        assert_eq!(
            MaterialDate::parse_compact("02/29/2024"),
            Some(MaterialDate::new(2024, 2, 29))
        );
    }

    // ---- ranges ---------------------------------------------------------

    #[test]
    fn a_closed_range_orders_its_ends() {
        let d = |day| MaterialDate::new(2026, 8, day);
        let forward = DateRange::closed(d(3), d(9));
        let reversed = DateRange::closed(d(9), d(3));
        assert_eq!(forward, reversed);
        assert!(forward.is_complete());
        assert!(!DateRange::new(d(3)).is_complete());
    }

    #[test]
    fn range_containment_is_inclusive_and_an_open_range_holds_only_its_start() {
        let d = |day| MaterialDate::new(2026, 8, day);
        let range = DateRange::closed(d(3), d(9));
        assert!(range.contains(d(3)));
        assert!(range.contains(d(6)));
        assert!(range.contains(d(9)));
        assert!(!range.contains(d(2)));
        assert!(!range.contains(d(10)));

        let open = DateRange::new(d(3));
        assert!(open.contains(d(3)));
        assert!(!open.contains(d(4)));
    }

    #[test]
    fn extend_closes_forward_and_restarts_backward() {
        let d = |day| MaterialDate::new(2026, 8, day);
        let open = DateRange::extend(None, d(3));
        assert_eq!(open, DateRange::new(d(3)));

        // Same day closes the range onto itself (`!date.isBefore(start)`).
        assert_eq!(DateRange::extend(Some(open), d(3)).end, Some(d(3)));
        assert_eq!(DateRange::extend(Some(open), d(9)).end, Some(d(9)));
        assert_eq!(DateRange::extend(Some(open), d(1)), DateRange::new(d(1)));

        let closed = DateRange::closed(d(3), d(9));
        assert_eq!(DateRange::extend(Some(closed), d(5)), DateRange::new(d(5)));
        assert_eq!(
            DateRange::extend(Some(closed), d(20)),
            DateRange::new(d(20))
        );
    }
}
