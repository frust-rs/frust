//! Time, date, and date-time ranges (`9:00 – 11:00 AM`), hand-rolled over
//! the ordinary formatters.
//!
//! # Why hand-rolled
//!
//! `icu_datetime` 2.2, `icu_experimental` 0.5 and `icu_time` 2.2 ship no
//! interval or range formatter, and the baked data has no interval key. CLDR
//! publishes `intervalFormats`, but nothing in the ICU4X crates consumes
//! them. So a range here is two ordinary formatted ends joined by a
//! per-language separator, with one string-level refinement (below).
//!
//! This is a **simplification**, recorded in `docs/LIMITATIONS.md` as
//! `i18n-interval-hand-rolled`: it reproduces CLDR's output for the common
//! slot-pill shapes, not every interval skeleton (no `Jan 5 – 9, 2024`
//! month/year elision for dates, no per-locale reordering of the period).
//!
//! # Separator table
//!
//! Keyed by the language subtag: `ja` uses `～` (U+FF5E, no spaces), `zh`
//! uses `–` (U+2013, no spaces), everything else — `en`, `es`, `vi`, `th`
//! and the default — uses ASCII space, U+2013, ASCII space.
//!
//! # Shared day-period collapse
//!
//! When the locale's time format is 12-hour and both ends carry the same day
//! period, the period is dropped from the first end and kept on the last:
//! `9:00 – 11:00 AM`, but `11:00 AM – 1:00 PM`. Whether a locale is 12-hour,
//! and on which side of the digits its period sits (`9:00 AM` versus
//! `上午9:00`), is probed once per locale by formatting 09:05 and 21:05 and
//! comparing the non-digit text around the numbers; 24-hour locales (de, ja,
//! th, vi) are never touched. The collapse is plain string post-processing.
//!
//! # Reversed ranges
//!
//! A range whose end precedes its start never panics and is never "fixed":
//! the two ends are formatted independently and joined in the order given,
//! with no collapse.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::{I18nError, Locale};

use super::{CivilDate, CivilTime, DateLength, datetime, zone};

/// Where a locale puts its day period relative to the digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// 24-hour: no day period.
    TwentyFour,
    /// `9:00 AM` — period after the digits.
    Suffix,
    /// `上午9:00` — period before the digits.
    Prefix,
}

static LAYOUTS: LazyLock<Mutex<HashMap<Locale, Layout>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The separator between the two ends, by language subtag.
fn separator(locale: &Locale) -> &'static str {
    match locale.language().as_str() {
        "ja" => "\u{ff5e}",
        "zh" => "\u{2013}",
        _ => " \u{2013} ",
    }
}

/// Splits a formatted time into (period, digits-only remainder) by layout.
fn split_period(text: &str, layout: Layout) -> Option<(&str, &str)> {
    match layout {
        Layout::TwentyFour => None,
        Layout::Suffix => {
            let (idx, c) = text.char_indices().rev().find(|(_, c)| c.is_numeric())?;
            let end = idx + c.len_utf8();
            Some((text[end..].trim(), &text[..end]))
        }
        Layout::Prefix => {
            let idx = text.find(|c: char| c.is_numeric())?;
            Some((text[..idx].trim(), &text[idx..]))
        }
    }
}

fn period_text(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_numeric() || c.is_whitespace() || c == ':' || c == '.')
}

/// Probes `locale`'s 12-hour layout once and caches it.
fn layout(locale: &Locale) -> Layout {
    if let Some(found) = LAYOUTS.lock().ok().and_then(|m| m.get(locale).copied()) {
        return found;
    }
    let probe = |hour| {
        let time = CivilTime {
            hour,
            minute: 5,
            second: 0,
        };
        datetime::time(locale, time, DateLength::Short).unwrap_or_default()
    };
    let (morning, evening) = (probe(9), probe(21));
    let layout = if morning.is_empty() || morning == evening {
        Layout::TwentyFour
    } else {
        let digits = |s: &str| s.chars().filter(|c| c.is_numeric()).collect::<String>();
        let at_start = !morning.starts_with(|c: char| c.is_numeric());
        let (m, e) = if at_start {
            (
                split_period(&morning, Layout::Prefix),
                split_period(&evening, Layout::Prefix),
            )
        } else {
            (
                split_period(&morning, Layout::Suffix),
                split_period(&evening, Layout::Suffix),
            )
        };
        match (m, e) {
            (Some((mp, _)), Some((ep, _)))
                if !mp.is_empty()
                    && !ep.is_empty()
                    && mp != ep
                    && digits(&morning) == digits(&evening) =>
            {
                if at_start {
                    Layout::Prefix
                } else {
                    Layout::Suffix
                }
            }
            _ => Layout::TwentyFour,
        }
    };
    if let Ok(mut map) = LAYOUTS.lock() {
        map.insert(locale.clone(), layout);
    }
    layout
}

/// Drops the shared day period from the first end (suffix layouts) or the
/// last end (prefix layouts); returns the pair unchanged when it differs.
fn collapse(layout: Layout, start: String, end: String) -> (String, String) {
    let shared = match (split_period(&start, layout), split_period(&end, layout)) {
        (Some((a, _)), Some((b, _))) => !a.is_empty() && period_text(a) == period_text(b),
        _ => false,
    };
    if !shared {
        return (start, end);
    }
    match layout {
        Layout::Suffix => {
            let kept = split_period(&start, layout).map(|(_, rest)| rest.to_owned());
            (kept.unwrap_or(start), end)
        }
        Layout::Prefix => {
            let kept = split_period(&end, layout).map(|(_, rest)| rest.to_owned());
            (start, kept.unwrap_or(end))
        }
        Layout::TwentyFour => (start, end),
    }
}

fn time_pair(
    locale: &Locale,
    start: CivilTime,
    end: CivilTime,
    length: DateLength,
) -> Result<String, I18nError> {
    let (a, b) = (
        datetime::time(locale, start, length)?,
        datetime::time(locale, end, length)?,
    );
    let ordered = (start.hour, start.minute, start.second) <= (end.hour, end.minute, end.second);
    let (a, b) = if ordered {
        collapse(layout(locale), a, b)
    } else {
        (a, b)
    };
    Ok(format!("{a}{}{b}", separator(locale)))
}

/// Formats a time-of-day range, e.g. `9:00 – 11:00 AM` in `en-US`.
///
/// Both ends are formatted by [`time`](super::time); see the module doc for
/// the separator table, the day-period collapse (a simplification recorded as
/// `i18n-interval-hand-rolled`), and the reversed-range behavior (`end <
/// start` is joined in the order given, uncollapsed, and never panics).
///
/// # Errors
///
/// As [`time`](super::time).
pub fn time_range(
    locale: &Locale,
    start: CivilTime,
    end: CivilTime,
    length: DateLength,
) -> Result<String, I18nError> {
    time_pair(locale, start, end, length)
}

/// Formats the wall-clock range between two instants in zone `iana`.
///
/// A range crossing local midnight reads as two times with no date (`11:00
/// PM – 1:00 AM`); use [`datetime_range_in_zone`] to show the dates.
///
/// # Errors
///
/// As [`zoned_civil`](super::zoned_civil) and [`time_range`].
pub fn time_range_in_zone(
    locale: &Locale,
    start_unix: i64,
    end_unix: i64,
    iana: &str,
    length: DateLength,
) -> Result<String, I18nError> {
    let (_, start) = zone::zoned_civil(start_unix, iana)?;
    let (_, end) = zone::zoned_civil(end_unix, iana)?;
    // Order by instant, not wall clock, so a cross-midnight range collapses
    // like any other forward range.
    let (a, b) = (
        datetime::time(locale, start, length)?,
        datetime::time(locale, end, length)?,
    );
    let (a, b) = if start_unix <= end_unix {
        collapse(layout(locale), a, b)
    } else {
        (a, b)
    };
    Ok(format!("{a}{}{b}", separator(locale)))
}

/// Formats a date range: both dates in full, joined by the separator.
///
/// No month or year elision is attempted (`Jan 5, 2024 – Jan 9, 2024`);
/// a reversed range is joined in the order given.
///
/// # Errors
///
/// As [`date`](super::date).
pub fn date_range(
    locale: &Locale,
    start: CivilDate,
    end: CivilDate,
    length: DateLength,
) -> Result<String, I18nError> {
    Ok(format!(
        "{}{}{}",
        datetime::date(locale, start, length)?,
        separator(locale),
        datetime::date(locale, end, length)?,
    ))
}

/// Formats the range between two instants in zone `iana`, with dates.
///
/// When both ends fall on the same local day the result is one date-time
/// whose time part is the [`time_range_in_zone`] range (`Nov 15, 2023, 9:00
/// – 11:00 AM`); otherwise it is two full date-times joined by the separator.
///
/// # Errors
///
/// As [`datetime_in_zone`](super::datetime_in_zone).
pub fn datetime_range_in_zone(
    locale: &Locale,
    start_unix: i64,
    end_unix: i64,
    iana: &str,
    length: DateLength,
) -> Result<String, I18nError> {
    let (start_date, start_time) = zone::zoned_civil(start_unix, iana)?;
    let (end_date, end_time) = zone::zoned_civil(end_unix, iana)?;
    let first = datetime::datetime(locale, start_date, start_time, length)?;

    if start_date != end_date || start_unix > end_unix {
        let last = datetime::datetime(locale, end_date, end_time, length)?;
        return Ok(format!("{first}{}{last}", separator(locale)));
    }

    let range = time_range_in_zone(locale, start_unix, end_unix, iana, length)?;
    let start_only = datetime::time(locale, start_time, length)?;
    Ok(match first.find(&start_only) {
        Some(at) => format!("{}{range}{}", &first[..at], &first[at + start_only.len()..]),
        None => format!("{} {range}", datetime::date(locale, start_date, length)?),
    })
}
