// Ported from `material_3_expressive` v1.0.8's day/year grid geometry (MIT,
// © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/date_pickers/utils/m3e_date_picker_utils.dart`
// — `dayPickerRowCount`/`calendarDayViewHeight`/`calendarYearViewHeight`/
// `yearPickerRowCount`/`yearPickerGridNaturalHeight`/`yearPickerGridHeight` —
// and `components/m3e_day_picker.dart`'s `firstDayOffset`/`cellCount`,
// retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>

//! Pure grid geometry for the calendar: which cell holds which day, how many
//! rows a month needs, and how tall each of the two sub-views is.
//!
//! Every function here is a total function of plain numbers — no theme, no
//! widget, no clock — which is what lets the layout rules be pinned directly
//! rather than inferred from a painted scene.

use super::date::{MaterialDate, days_in_month};
use super::{
    DAY_GRID_TOP_PADDING, DAY_ROW_HEIGHT, DAYS_PER_WEEK, MAX_DAY_PICKER_HEIGHT, SUB_HEADER_HEIGHT,
    WEEKDAY_ROW_HEIGHT, YEAR_COLUMN_COUNT, YEAR_GRID_PADDING, YEAR_ROW_HEIGHT, YEAR_ROW_SPACING,
};

/// One month's 7-column day grid: how many blank leading cells precede the 1st,
/// how many days the month has, and therefore how many week rows it occupies.
///
/// The reference computes the same three numbers inline in
/// `M3EDayPicker.build`; they are lifted into a value type here so the grid
/// layout can be asserted without painting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonthGrid {
    year: i32,
    month: u32,
    leading_blanks: usize,
    days: usize,
}

impl MonthGrid {
    /// The grid for `month`'s own month (its day component is ignored), with
    /// the week starting on `first_day_of_week` (`0` Sunday .. `6` Saturday;
    /// out-of-range values fold with `% 7`).
    ///
    /// `leading_blanks` is the reference's `firstDayOffset`, `(weekday -
    /// firstDayOfWeekIndex) % 7`, expressed over
    /// [`MaterialDate::weekday_index`]'s already Sunday-folded index.
    pub fn new(month: MaterialDate, first_day_of_week: u32) -> Self {
        let first = month.first_of_month();
        let start = first.weekday_index();
        let offset = first_day_of_week % DAYS_PER_WEEK as u32;
        Self {
            year: first.year(),
            month: first.month(),
            leading_blanks: ((start + DAYS_PER_WEEK as u32 - offset) % DAYS_PER_WEEK as u32)
                as usize,
            days: days_in_month(first.year(), first.month()) as usize,
        }
    }

    /// How many blank cells precede the 1st.
    pub fn leading_blanks(self) -> usize {
        self.leading_blanks
    }

    /// How many days this month has.
    pub fn days(self) -> usize {
        self.days
    }

    /// How many week rows the grid occupies — `ceil((days + blanks) / 7)`,
    /// always `4..=6` for a real month.
    pub fn rows(self) -> usize {
        self.leading_blanks
            .saturating_add(self.days)
            .div_ceil(DAYS_PER_WEEK)
    }

    /// How many cells the grid holds, blanks included (`rows * 7`).
    pub fn cell_count(self) -> usize {
        self.rows() * DAYS_PER_WEEK
    }

    /// The date in cell `index` (row-major from the top-left), or `None` for a
    /// leading blank or a trailing one past the month's end.
    pub fn date_at(self, index: usize) -> Option<MaterialDate> {
        if index < self.leading_blanks {
            return None;
        }
        let day = index - self.leading_blanks + 1;
        if day > self.days {
            return None;
        }
        Some(MaterialDate::new(self.year, self.month, day as u32))
    }

    /// The cell index holding `day` (`1..=`[`Self::days`]), or `None` if the
    /// month is not that long.
    pub fn index_of(self, day: u32) -> Option<usize> {
        let day = day as usize;
        if day < 1 || day > self.days {
            return None;
        }
        Some(self.leading_blanks + day - 1)
    }
}

/// How many rows a year grid of `count` years occupies (3 per row) — the
/// reference's `yearPickerRowCount`.
pub fn year_row_count(count: usize) -> usize {
    count.div_ceil(YEAR_COLUMN_COUNT)
}

/// The unbounded height a year grid of `count` years wants — the reference's
/// `yearPickerGridNaturalHeight` (padding on both ends, one
/// [`YEAR_ROW_HEIGHT`] per row, [`YEAR_ROW_SPACING`] between rows).
pub fn year_grid_natural_height(count: usize) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let rows = year_row_count(count) as f64;
    YEAR_GRID_PADDING * 2.0 + rows * YEAR_ROW_HEIGHT + (rows - 1.0) * YEAR_ROW_SPACING
}

/// The year grid's on-screen height: its natural height capped at
/// [`MAX_DAY_PICKER_HEIGHT`] — the reference's `yearPickerGridHeight`. A grid
/// taller than the cap scrolls inside it.
pub fn year_grid_height(count: usize) -> f64 {
    year_grid_natural_height(count).min(MAX_DAY_PICKER_HEIGHT)
}

/// Whether a year grid of `count` years overflows its capped viewport, i.e.
/// whether it scrolls at all — the reference's `scrollable` flag.
pub fn year_grid_scrolls(count: usize) -> bool {
    year_grid_natural_height(count) > year_grid_height(count)
}

/// The day sub-view's height for `month` — the reference's
/// `calendarDayViewHeight`: sub-header, weekday row, one row per week, plus the
/// grid's own top padding.
pub fn calendar_day_view_height(month: MaterialDate, first_day_of_week: u32) -> f64 {
    let rows = MonthGrid::new(month, first_day_of_week).rows() as f64;
    SUB_HEADER_HEIGHT + WEEKDAY_ROW_HEIGHT + DAY_ROW_HEIGHT * rows + DAY_GRID_TOP_PADDING
}

/// The year sub-view's height for a `first..=last` span — the reference's
/// `calendarYearViewHeight`. `include_sub_header` adds the month/year toggle
/// band above the grid (the dialog includes it; an inline calendar that already
/// paints its own toggle does not).
pub fn calendar_year_view_height(
    first: MaterialDate,
    last: MaterialDate,
    include_sub_header: bool,
) -> f64 {
    let mut height = year_grid_height(year_span(first, last));
    if include_sub_header {
        height += SUB_HEADER_HEIGHT;
    }
    height
}

/// How many years a `first..=last` span covers, inclusive — `0` for a reversed
/// pair (the degenerate case a caller can hand in; the reference asserts
/// instead).
pub fn year_span(first: MaterialDate, last: MaterialDate) -> usize {
    if last.year() < first.year() {
        return 0;
    }
    (last.year() - first.year() + 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sunday-first, the `en_US` `firstDayOfWeekIndex`.
    const SUNDAY: u32 = 0;
    /// Monday-first, the `en_GB`/ISO shape — the override an app supplies via
    /// `DatePickerStrings::first_day_of_week`.
    const MONDAY: u32 = 1;

    // ---- leading blanks / rows, pinned per month ------------------------

    #[test]
    fn three_months_with_different_starting_weekdays_lay_out_as_pinned() {
        // Sunday-first weeks. February 2026 starts on a Sunday, March 2026 on
        // a Sunday too, so pick three months whose 1st falls on three
        // different weekdays:
        //   2026-02-01 Sunday   (blanks 0, 28 days -> 4 rows exactly)
        //   2026-05-01 Friday   (blanks 5, 31 days -> 6 rows)
        //   2026-09-01 Tuesday  (blanks 2, 30 days -> 5 rows)
        let feb = MonthGrid::new(MaterialDate::new(2026, 2, 14), SUNDAY);
        assert_eq!(feb.leading_blanks(), 0);
        assert_eq!(feb.days(), 28);
        assert_eq!(feb.rows(), 4, "a 28-day month starting on the first column");
        assert_eq!(feb.cell_count(), 28);

        let may = MonthGrid::new(MaterialDate::new(2026, 5, 1), SUNDAY);
        assert_eq!(may.leading_blanks(), 5);
        assert_eq!(may.days(), 31);
        assert_eq!(may.rows(), 6);
        assert_eq!(may.cell_count(), 42);

        let sep = MonthGrid::new(MaterialDate::new(2026, 9, 30), SUNDAY);
        assert_eq!(sep.leading_blanks(), 2);
        assert_eq!(sep.days(), 30);
        assert_eq!(sep.rows(), 5);
        assert_eq!(sep.cell_count(), 35);
    }

    #[test]
    fn a_leap_february_gains_a_row_when_it_starts_late_enough() {
        // 2024-02-01 was a Thursday (blanks 4) with 29 days -> 33 cells -> 5 rows.
        let leap = MonthGrid::new(MaterialDate::new(2024, 2, 1), SUNDAY);
        assert_eq!(leap.leading_blanks(), 4);
        assert_eq!(leap.days(), 29);
        assert_eq!(leap.rows(), 5);

        // 2021-02-01 was a Monday (blanks 1) with 28 days -> 29 cells -> 5 rows.
        let common = MonthGrid::new(MaterialDate::new(2021, 2, 1), SUNDAY);
        assert_eq!(common.leading_blanks(), 1);
        assert_eq!(common.days(), 28);
        assert_eq!(common.rows(), 5);
    }

    #[test]
    fn the_first_day_of_week_rotates_the_leading_blanks() {
        // 2026-05-01 is a Friday: 5 blanks Sunday-first, 4 Monday-first.
        let may = MaterialDate::new(2026, 5, 1);
        assert_eq!(MonthGrid::new(may, SUNDAY).leading_blanks(), 5);
        assert_eq!(MonthGrid::new(may, MONDAY).leading_blanks(), 4);
        // 2026-02-01 is a Sunday: 0 blanks Sunday-first, 6 Monday-first (which
        // pushes it from 4 rows to 5).
        let feb = MaterialDate::new(2026, 2, 1);
        assert_eq!(MonthGrid::new(feb, SUNDAY).leading_blanks(), 0);
        assert_eq!(MonthGrid::new(feb, SUNDAY).rows(), 4);
        assert_eq!(MonthGrid::new(feb, MONDAY).leading_blanks(), 6);
        assert_eq!(MonthGrid::new(feb, MONDAY).rows(), 5);
    }

    #[test]
    fn an_out_of_range_first_day_of_week_folds_rather_than_panicking() {
        let may = MaterialDate::new(2026, 5, 1);
        assert_eq!(
            MonthGrid::new(may, 7).leading_blanks(),
            MonthGrid::new(may, SUNDAY).leading_blanks()
        );
        assert_eq!(
            MonthGrid::new(may, 15).leading_blanks(),
            MonthGrid::new(may, MONDAY).leading_blanks()
        );
    }

    #[test]
    fn every_month_of_a_year_needs_between_four_and_six_rows() {
        for year in [1900, 2000, 2024, 2026] {
            for month in 1..=12 {
                for start in 0..7 {
                    let grid = MonthGrid::new(MaterialDate::new(year, month, 1), start);
                    let rows = grid.rows();
                    assert!(
                        (4..=6).contains(&rows),
                        "{year}-{month:02} at start {start} wanted {rows} rows"
                    );
                }
            }
        }
    }

    // ---- cell mapping ---------------------------------------------------

    #[test]
    fn cells_map_to_days_and_back_with_blanks_at_both_ends() {
        // May 2026: 5 blanks, then 1..=31, then 6 trailing blanks (42 cells).
        let grid = MonthGrid::new(MaterialDate::new(2026, 5, 1), SUNDAY);
        for i in 0..5 {
            assert_eq!(grid.date_at(i), None, "cell {i} is a leading blank");
        }
        assert_eq!(grid.date_at(5), Some(MaterialDate::new(2026, 5, 1)));
        assert_eq!(grid.date_at(35), Some(MaterialDate::new(2026, 5, 31)));
        for i in 36..grid.cell_count() {
            assert_eq!(grid.date_at(i), None, "cell {i} is a trailing blank");
        }
        assert_eq!(grid.date_at(grid.cell_count()), None, "past the last cell");

        assert_eq!(grid.index_of(1), Some(5));
        assert_eq!(grid.index_of(31), Some(35));
        assert_eq!(grid.index_of(32), None);
        assert_eq!(grid.index_of(0), None);
    }

    #[test]
    fn date_at_and_index_of_are_inverses_for_every_real_day() {
        for month in 1..=12 {
            let grid = MonthGrid::new(MaterialDate::new(2026, month, 1), SUNDAY);
            for day in 1..=grid.days() as u32 {
                let index = grid.index_of(day).expect("a real day has a cell");
                assert_eq!(
                    grid.date_at(index),
                    Some(MaterialDate::new(2026, month, day))
                );
            }
        }
    }

    #[test]
    fn every_row_holds_exactly_seven_cells_and_the_days_are_contiguous() {
        let grid = MonthGrid::new(MaterialDate::new(2026, 9, 1), SUNDAY);
        let mut seen = Vec::new();
        for i in 0..grid.cell_count() {
            if let Some(date) = grid.date_at(i) {
                seen.push(date.day());
            }
        }
        assert_eq!(seen, (1..=30).collect::<Vec<u32>>());
        assert_eq!(grid.cell_count() % DAYS_PER_WEEK, 0);
    }

    // ---- year grid ------------------------------------------------------

    #[test]
    fn the_year_grid_packs_three_per_row() {
        assert_eq!(year_row_count(0), 0);
        assert_eq!(year_row_count(1), 1);
        assert_eq!(year_row_count(3), 1);
        assert_eq!(year_row_count(4), 2);
        assert_eq!(year_row_count(21), 7);
        assert_eq!(year_row_count(22), 8);
    }

    #[test]
    fn the_year_grid_height_is_capped_and_reports_when_it_scrolls() {
        // One row: 16 + 52 + 16 = 84.
        assert_eq!(year_grid_natural_height(3), 84.0);
        // Two rows: 16 + 52 + 8 + 52 + 16 = 144.
        assert_eq!(year_grid_natural_height(6), 144.0);
        assert_eq!(year_grid_natural_height(0), 0.0);
        assert_eq!(year_grid_height(3), 84.0);
        assert!(!year_grid_scrolls(3));

        // The cap is 48 * 7 = 336; a natural height past it clips and scrolls.
        assert!(year_grid_natural_height(30) > MAX_DAY_PICKER_HEIGHT);
        assert_eq!(year_grid_height(30), MAX_DAY_PICKER_HEIGHT);
        assert!(year_grid_scrolls(30));
    }

    #[test]
    fn the_year_span_is_inclusive_and_a_reversed_pair_is_empty() {
        assert_eq!(
            year_span(
                MaterialDate::new(2026, 1, 1),
                MaterialDate::new(2026, 12, 31)
            ),
            1
        );
        assert_eq!(
            year_span(MaterialDate::new(2020, 6, 1), MaterialDate::new(2030, 6, 1)),
            11
        );
        assert_eq!(
            year_span(MaterialDate::new(2030, 1, 1), MaterialDate::new(2020, 1, 1)),
            0
        );
    }

    // ---- sub-view heights ------------------------------------------------

    #[test]
    fn the_day_view_height_tracks_the_month_row_count() {
        // 52 + 24 + 48 * rows + 4.
        let four_rows = calendar_day_view_height(MaterialDate::new(2026, 2, 1), SUNDAY);
        let five_rows = calendar_day_view_height(MaterialDate::new(2026, 9, 1), SUNDAY);
        let six_rows = calendar_day_view_height(MaterialDate::new(2026, 5, 1), SUNDAY);
        assert_eq!(four_rows, 52.0 + 24.0 + 48.0 * 4.0 + 4.0);
        assert_eq!(five_rows, 52.0 + 24.0 + 48.0 * 5.0 + 4.0);
        assert_eq!(six_rows, 52.0 + 24.0 + 48.0 * 6.0 + 4.0);
        assert!(four_rows < five_rows && five_rows < six_rows);
    }

    #[test]
    fn the_year_view_height_optionally_carries_the_sub_header() {
        let first = MaterialDate::new(2020, 1, 1);
        let last = MaterialDate::new(2030, 12, 31);
        let bare = calendar_year_view_height(first, last, false);
        let with_header = calendar_year_view_height(first, last, true);
        assert_eq!(bare, year_grid_height(11));
        assert_eq!(with_header - bare, SUB_HEADER_HEIGHT);
    }
}
