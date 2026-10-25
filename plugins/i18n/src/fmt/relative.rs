//! "Today" / "Tomorrow" / "Yesterday" for a calendar day, in the venue zone.
//!
//! The day of an instant is the civil date it reads as in the venue's IANA
//! zone ([`zone::zoned_civil`]), never the device's: 23:30 and 00:30 local
//! are different days even when the same minute apart. The distance between
//! two days is a whole number of civil days. A distance of at most one day
//! goes through ICU4X's `RelativeTimeFormatter` with `Numeric::Auto` (so
//! `0`, `1`, `-1` become the locale's own words); anything further away
//! falls back to a medium date, because "in 2 days" is not what a booking
//! row wants.
//!
//! # Capitalisation
//!
//! The string is ICU's, returned as-is. CLDR capitalises per width in some
//! languages (Vietnamese is "hôm nay" short but "Hôm nay" long), and the
//! word is lower case in most languages mid-sentence. Sentence-casing for a
//! standalone label is the caller's concern.

use std::sync::LazyLock;

use icu_decimal::input::Decimal;
use icu_experimental::relativetime::options::{Numeric, RelativeTimeFormatterOptions};
use icu_experimental::relativetime::{RelativeTimeFormatter, RelativeTimeFormatterPreferences};

use crate::{I18nError, Locale};

use super::bridge::{self, Cache};
use super::{CivilDate, CivilTime, DateLength, datetime, zone};

const SECONDS_PER_DAY: i64 = 86_400;

/// Which CLDR width of relative-day wording to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelativeDayStyle {
    /// The long form ("Hôm nay" in Vietnamese).
    Long,
    /// The short form ("hôm nay" in Vietnamese).
    Short,
}

type RelativeCache = Cache<(Locale, RelativeDayStyle), RelativeTimeFormatter>;

static RELATIVE: LazyLock<RelativeCache> = LazyLock::new(Cache::new);

/// `instant_unix` as a day relative to `now_unix`, both read in zone `iana`:
/// the locale's word for today/tomorrow/yesterday, else a medium date.
///
/// # Errors
///
/// [`I18nError::UnknownTimeZone`] for an unknown zone; [`I18nError::Format`]
/// for an out-of-range instant or when the locale's data fails to load.
pub fn relative_day(
    locale: &Locale,
    instant_unix: i64,
    now_unix: i64,
    iana: &str,
    style: RelativeDayStyle,
) -> Result<String, I18nError> {
    let (day, _) = zone::zoned_civil(instant_unix, iana)?;
    let (today, _) = zone::zoned_civil(now_unix, iana)?;
    relative_day_civil(locale, day, today, style)
}

/// As [`relative_day`], for callers that already hold the two civil dates.
///
/// # Errors
///
/// [`I18nError::Format`] when a date is not a real date or the locale's data
/// fails to load.
pub fn relative_day_civil(
    locale: &Locale,
    day: CivilDate,
    today: CivilDate,
    style: RelativeDayStyle,
) -> Result<String, I18nError> {
    let delta = day_number(day)? - day_number(today)?;
    if !(-1..=1).contains(&delta) {
        return datetime::date(locale, day, DateLength::Medium);
    }

    let formatter = RELATIVE
        .get_or_build(&(locale.clone(), style), || {
            let mut options = RelativeTimeFormatterOptions::default();
            options.numeric = Some(Numeric::Auto);
            let prefs = RelativeTimeFormatterPreferences::from(&bridge::lang_id(locale));
            match style {
                RelativeDayStyle::Long => RelativeTimeFormatter::try_new_long_day(prefs, options),
                RelativeDayStyle::Short => RelativeTimeFormatter::try_new_short_day(prefs, options),
            }
        })
        .map_err(|error| {
            I18nError::Format(format!(
                "relative-day data for `{locale}` failed to load: {error}"
            ))
        })?;

    Ok(formatter.format(Decimal::from(delta)).to_string())
}

/// Whole days since 1970-01-01 of a civil date.
fn day_number(date: CivilDate) -> Result<i64, I18nError> {
    let midnight = CivilTime {
        hour: 0,
        minute: 0,
        second: 0,
    };
    zone::utc_unix(date, midnight)
        .map(|seconds| seconds.div_euclid(SECONDS_PER_DAY))
        .ok_or_else(|| I18nError::Format(format!("{date:?} is not a real date")))
}
