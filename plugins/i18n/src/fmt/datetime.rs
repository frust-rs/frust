//! Locale-aware date, time, and date-time formatting.
//!
//! # Semantic field sets, not a skeleton language
//!
//! ICU4X 2.x formats through UTS-35 *semantic skeletons*: a field set
//! (`YMD` = year + month + day, `T` = time, `YMDT` = both) plus a length.
//! This module exposes exactly three shapes per kind — [`DateLength`]'s
//! `Short`/`Medium`/`Long` over `YMD`/`T`/`YMDT` — mirroring the shapes Dart's
//! `DateFormat` callers actually reach for, rather than surfacing the whole
//! skeleton vocabulary. A caller needing `MMMM y` or a weekday builds it from
//! a Fluent message, or this module grows a field set later; the length
//! ladder is deliberately not the extension point.
//!
//! `Long` adds seconds to a time the way `Medium` does; the difference
//! between them is the *date* half's spelling, which is CLDR's call, not
//! this module's.
//!
//! # Calendar: the locale's, with a proleptic-Gregorian input
//!
//! Input is a plain proleptic-Gregorian (ISO 8601) year/month/day —
//! [`super::CivilDate`] — and output is rendered in the calendar CLDR gives
//! the locale, which is Gregorian for every locale but a handful (`th` is
//! Buddhist, `fa` Persian). That is ICU4X's own recommendation and the
//! reason [`DateTimeFormatter`] exists at all; pinning Gregorian output
//! instead would need `FixedCalendarDateTimeFormatter<Gregorian, _>`, whose
//! calendar type `icu_datetime` does not re-export.
//!
//! A [`crate::Locale`] carries no `-u-ca-`/`-u-hc-` extension (it is a
//! *language identifier*), so neither the calendar nor the hour cycle can be
//! overridden per call — both are whatever CLDR gives the locale.
//!
//! # Time zones: none
//!
//! Every input is a civil (wall-clock) date-time with no zone, and no field
//! set here contains a zone field, so nothing is ever converted or labelled
//! with an offset. The one place an instant enters this crate is
//! [`from_unix_seconds`], which resolves it in **UTC**.

use std::sync::LazyLock;

use icu_datetime::fieldsets::{T, YMD, YMDT};
use icu_datetime::input::{Date, DateTime, Time};
use icu_datetime::options::TimePrecision;
use icu_datetime::{DateTimeFormatter, DateTimeFormatterPreferences, NoCalendarFormatter};

use crate::{I18nError, Locale};

use super::bridge::{self, Cache};
use super::{CivilDate, CivilTime, DateLength};

/// Seconds in a civil day — the divisor [`from_unix_seconds`] splits on.
const SECONDS_PER_DAY: i64 = 86_400;

/// Days from 0000-03-01 (the algorithm's shifted epoch) to 1970-01-01.
///
/// From Howard Hinnant's `civil_from_days`
/// (<https://howardhinnant.github.io/date_algorithms.html>), the public
/// derivation this module's epoch → civil conversion follows.
const DAYS_TO_UNIX_EPOCH: i64 = 719_468;

/// Days in a 400-year Gregorian era.
const DAYS_PER_ERA: i64 = 146_097;

type DateCache = Cache<(Locale, DateLength), DateTimeFormatter<YMD>>;
type TimeCache = Cache<(Locale, DateLength), NoCalendarFormatter<T>>;
type DateTimeCache = Cache<(Locale, DateLength), DateTimeFormatter<YMDT>>;

static DATES: LazyLock<DateCache> = LazyLock::new(Cache::new);
static TIMES: LazyLock<TimeCache> = LazyLock::new(Cache::new);
static DATE_TIMES: LazyLock<DateTimeCache> = LazyLock::new(Cache::new);

/// Formats `date` at `length` in `locale`'s calendar.
pub(super) fn date(
    locale: &Locale,
    date: CivilDate,
    length: DateLength,
) -> Result<String, I18nError> {
    let value =
        Date::try_new_iso(date.year, date.month, date.day).map_err(|e| bad_date(date, e))?;
    let formatter = DATES
        .get_or_build(&(locale.clone(), length), || {
            DateTimeFormatter::try_new(prefs(locale), date_fields(length))
        })
        .map_err(|error| load_failure(locale, "date", &error))?;

    Ok(formatter.format(&value).to_string())
}

/// Formats `time` at `length` in `locale`'s hour cycle.
pub(super) fn time(
    locale: &Locale,
    time: CivilTime,
    length: DateLength,
) -> Result<String, I18nError> {
    let value =
        Time::try_new(time.hour, time.minute, time.second, 0).map_err(|e| bad_time(time, e))?;
    let formatter = TIMES
        .get_or_build(&(locale.clone(), length), || {
            NoCalendarFormatter::try_new(prefs(locale), time_fields(length))
        })
        .map_err(|error| load_failure(locale, "time", &error))?;

    Ok(formatter.format(&value).to_string())
}

/// Formats `date` and `time` together at `length`.
pub(super) fn datetime(
    locale: &Locale,
    date: CivilDate,
    time: CivilTime,
    length: DateLength,
) -> Result<String, I18nError> {
    let value = DateTime {
        date: Date::try_new_iso(date.year, date.month, date.day).map_err(|e| bad_date(date, e))?,
        time: Time::try_new(time.hour, time.minute, time.second, 0)
            .map_err(|e| bad_time(time, e))?,
    };
    let formatter = DATE_TIMES
        .get_or_build(&(locale.clone(), length), || {
            DateTimeFormatter::try_new(prefs(locale), datetime_fields(length))
        })
        .map_err(|error| load_failure(locale, "date-time", &error))?;

    Ok(formatter.format(&value).to_string())
}

/// Splits a Unix timestamp into the UTC civil date and time it names.
///
/// `None` when the timestamp is so far outside the common era that its year
/// does not fit an `i32` — unreachable for any `i64` a clock produces, but
/// the arm exists so no input can panic.
pub(super) fn from_unix_seconds(seconds: i64) -> Option<(CivilDate, CivilTime)> {
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);

    let date = civil_from_days(days)?;
    let time = CivilTime {
        hour: (second_of_day / 3_600) as u8,
        minute: ((second_of_day % 3_600) / 60) as u8,
        second: (second_of_day % 60) as u8,
    };

    Some((date, time))
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch → a
/// proleptic-Gregorian year/month/day
/// (<https://howardhinnant.github.io/date_algorithms.html>).
///
/// The `+ DAYS_TO_UNIX_EPOCH` shift moves the count onto an era boundary
/// (0000-03-01) so that March-based month arithmetic makes every era
/// identical, leap days included.
fn civil_from_days(days: i64) -> Option<CivilDate> {
    let shifted = days + DAYS_TO_UNIX_EPOCH;
    let era = shifted.div_euclid(DAYS_PER_ERA);
    // Day of era, always in [0, 146096] — every division below is on a
    // non-negative value, so plain integer division truncates as intended.
    let day_of_era = shifted.rem_euclid(DAYS_PER_ERA);

    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Month index counting from March, in [0, 11].
    let march_month = (5 * day_of_year + 2) / 153;

    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    Some(CivilDate {
        year: i32::try_from(year).ok()?,
        month: month as u8,
        day: day as u8,
    })
}

/// The ICU4X preferences bag for `locale`.
fn prefs(locale: &Locale) -> DateTimeFormatterPreferences {
    DateTimeFormatterPreferences::from(&bridge::lang_id(locale))
}

/// The date field set for `length`.
fn date_fields(length: DateLength) -> YMD {
    match length {
        DateLength::Short => YMD::short(),
        DateLength::Medium => YMD::medium(),
        DateLength::Long => YMD::long(),
    }
}

/// The time field set for `length`: minutes at `Short`, seconds above it.
///
/// The precision is pinned rather than left to the field set's default,
/// which `icu_datetime`'s own docs flag as subject to change.
fn time_fields(length: DateLength) -> T {
    match length {
        DateLength::Short => T::short().with_time_precision(TimePrecision::Minute),
        DateLength::Medium => T::medium().with_time_precision(TimePrecision::Second),
        DateLength::Long => T::long().with_time_precision(TimePrecision::Second),
    }
}

/// The combined field set for `length`, pairing [`date_fields`]'s date half
/// with [`time_fields`]'s precision.
fn datetime_fields(length: DateLength) -> YMDT {
    match length {
        DateLength::Short => YMD::short().with_time_hm(),
        DateLength::Medium => YMD::medium().with_time_hms(),
        DateLength::Long => YMD::long().with_time_hms(),
    }
}

/// Reports an out-of-range date field as a typed error rather than panicking.
fn bad_date(date: CivilDate, error: impl std::fmt::Display) -> I18nError {
    I18nError::Format(format!(
        "{}-{}-{} is not a date: {error}",
        date.year, date.month, date.day
    ))
}

/// Reports an out-of-range time field as a typed error.
fn bad_time(time: CivilTime, error: impl std::fmt::Display) -> I18nError {
    I18nError::Format(format!(
        "{}:{}:{} is not a time: {error}",
        time.hour, time.minute, time.second
    ))
}

/// Renders a formatter-construction failure as this crate's error.
fn load_failure(locale: &Locale, kind: &str, error: &impl std::fmt::Display) -> I18nError {
    I18nError::Format(format!("{locale}: loading {kind} data: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEW_YEAR: CivilDate = CivilDate {
        year: 2024,
        month: 1,
        day: 31,
    };
    const AFTERNOON: CivilTime = CivilTime {
        hour: 15,
        minute: 47,
        second: 50,
    };

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    #[test]
    fn a_medium_date_matches_what_icu_itself_renders() {
        for tag in ["en-US", "de-DE", "fr-FR", "ja-JP"] {
            let expected = DateTimeFormatter::try_new(
                DateTimeFormatterPreferences::from(&bridge::lang_id(&locale(tag))),
                YMD::medium(),
            )
            .expect("icu builds")
            .format(&Date::try_new_iso(2024, 1, 31).expect("valid date"))
            .to_string();

            assert_eq!(
                date(&locale(tag), NEW_YEAR, DateLength::Medium).expect("formats"),
                expected,
                "{tag}"
            );
        }
    }

    #[test]
    fn the_three_lengths_are_distinguishable() {
        let en = locale("en-US");
        let short = date(&en, NEW_YEAR, DateLength::Short).expect("formats");
        let medium = date(&en, NEW_YEAR, DateLength::Medium).expect("formats");
        let long = date(&en, NEW_YEAR, DateLength::Long).expect("formats");

        assert!(short.len() < medium.len(), "{short} / {medium}");
        assert!(medium.len() < long.len(), "{medium} / {long}");
    }

    #[test]
    fn date_spelling_differs_across_locales() {
        let en = date(&locale("en-US"), NEW_YEAR, DateLength::Long).expect("formats");
        let de = date(&locale("de-DE"), NEW_YEAR, DateLength::Long).expect("formats");

        assert_ne!(en, de);
        assert!(en.contains("2024") && de.contains("2024"), "{en} / {de}");
    }

    #[test]
    fn short_time_drops_seconds_and_medium_keeps_them() {
        let en = locale("en-US");

        assert!(
            !time(&en, AFTERNOON, DateLength::Short)
                .expect("formats")
                .contains("50")
        );
        assert!(
            time(&en, AFTERNOON, DateLength::Medium)
                .expect("formats")
                .contains("50")
        );
    }

    #[test]
    fn the_hour_cycle_follows_the_locale() {
        let us = time(&locale("en-US"), AFTERNOON, DateLength::Short).expect("formats");
        let de = time(&locale("de-DE"), AFTERNOON, DateLength::Short).expect("formats");

        assert!(us.contains('3'), "{us}");
        assert!(de.contains("15"), "{de}");
    }

    #[test]
    fn a_datetime_carries_both_halves() {
        let combined =
            datetime(&locale("en-US"), NEW_YEAR, AFTERNOON, DateLength::Medium).expect("formats");

        assert!(combined.contains("2024"), "{combined}");
        assert!(combined.contains("47"), "{combined}");
    }

    #[test]
    fn an_impossible_date_is_a_typed_error() {
        let error = date(
            &locale("en-US"),
            CivilDate {
                year: 2024,
                month: 13,
                day: 1,
            },
            DateLength::Short,
        )
        .expect_err("month 13");

        assert!(matches!(error, I18nError::Format(_)), "{error:?}");
    }

    #[test]
    fn an_impossible_time_is_a_typed_error() {
        let error = time(
            &locale("en-US"),
            CivilTime {
                hour: 25,
                minute: 0,
                second: 0,
            },
            DateLength::Short,
        )
        .expect_err("hour 25");

        assert!(matches!(error, I18nError::Format(_)), "{error:?}");
    }

    #[test]
    fn unix_seconds_resolve_to_the_utc_civil_datetime() {
        let cases = [
            (0_i64, (1970, 1, 1), (0, 0, 0)),
            (951_782_400, (2000, 2, 29), (0, 0, 0)),
            (1_706_715_270, (2024, 1, 31), (15, 34, 30)),
            (-1, (1969, 12, 31), (23, 59, 59)),
        ];

        for (seconds, (year, month, day), (hour, minute, second)) in cases {
            let (date, time) = from_unix_seconds(seconds).expect("in range");

            assert_eq!(
                (date.year, date.month, date.day),
                (year, month, day),
                "{seconds}"
            );
            assert_eq!(
                (time.hour, time.minute, time.second),
                (hour, minute, second),
                "{seconds}"
            );
        }
    }
}
