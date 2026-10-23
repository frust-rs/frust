//! Venue-zone formatting: an instant rendered in an IANA zone or at a fixed
//! offset, through the same formatters the zone-less API uses.
//!
//! # Zone lookup is jiff's; formatting is ICU4X's
//!
//! `jiff` (bundled tzdb, so no OS zone database is read and the result is the
//! same on every platform) answers exactly one question — "what is the UTC
//! offset of zone `Z` at instant `T`" — which makes DST and historical rule
//! changes correct. The instant is then shifted by that offset and split with
//! [`datetime::from_unix_seconds`], and the resulting civil pair goes through
//! [`datetime`]'s ordinary formatters. No zone field is ever rendered: the
//! output is the wall-clock reading in the zone, nothing more.
//!
//! The nonexistent/ambiguous local times of a DST gap never arise, because
//! the input is an instant (unambiguous) and the output a civil reading.

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::{I18nError, Locale};

use super::datetime::DateTimeOptions;
use super::{CivilDate, CivilTime, DateLength, datetime};

/// The UTC offset, in seconds east of UTC, that IANA zone `iana` has at the
/// instant `unix_seconds`.
///
/// # Errors
///
/// [`I18nError::UnknownTimeZone`] when `iana` names no zone in the bundled
/// database; [`I18nError::Format`] when `unix_seconds` is outside the range
/// the zone database represents.
pub fn zone_offset_seconds(unix_seconds: i64, iana: &str) -> Result<i32, I18nError> {
    let zone = TimeZone::get(iana).map_err(|_| I18nError::UnknownTimeZone(iana.to_owned()))?;
    let instant = Timestamp::from_second(unix_seconds)
        .map_err(|error| I18nError::Format(format!("instant {unix_seconds}: {error}")))?;

    Ok(zone.to_offset(instant).seconds())
}

/// The civil date and time the instant `unix_seconds` reads as in zone `iana`.
///
/// # Errors
///
/// As [`zone_offset_seconds`].
pub fn zoned_civil(unix_seconds: i64, iana: &str) -> Result<(CivilDate, CivilTime), I18nError> {
    civil_at_offset(unix_seconds, zone_offset_seconds(unix_seconds, iana)?)
}

/// The civil date and time the instant `unix_seconds` reads as at a fixed
/// `offset_seconds` east of UTC.
///
/// # Errors
///
/// [`I18nError::Format`] when the shifted instant is outside the supported
/// year range.
pub fn civil_at_offset(
    unix_seconds: i64,
    offset_seconds: i32,
) -> Result<(CivilDate, CivilTime), I18nError> {
    unix_seconds
        .checked_add(i64::from(offset_seconds))
        .and_then(datetime::from_unix_seconds)
        .ok_or_else(|| I18nError::Format(format!("instant {unix_seconds} is out of range")))
}

/// Formats the instant `unix_seconds` as a date-and-time in zone `iana`.
///
/// # Errors
///
/// [`zoned_civil`]'s, plus [`datetime`](super::datetime)'s.
pub fn datetime_in_zone(
    locale: &Locale,
    unix_seconds: i64,
    iana: &str,
    length: DateLength,
) -> Result<String, I18nError> {
    datetime_in_zone_with(
        locale,
        unix_seconds,
        iana,
        length,
        &DateTimeOptions::default(),
    )
}

/// [`datetime_in_zone`] with `options` overriding the calendar and hour cycle.
///
/// # Errors
///
/// As [`datetime_in_zone`].
pub fn datetime_in_zone_with(
    locale: &Locale,
    unix_seconds: i64,
    iana: &str,
    length: DateLength,
    options: &DateTimeOptions,
) -> Result<String, I18nError> {
    let (date, time) = zoned_civil(unix_seconds, iana)?;
    datetime::datetime_with(locale, date, time, length, options)
}

/// Formats the date the instant `unix_seconds` falls on in zone `iana`.
///
/// # Errors
///
/// As [`datetime_in_zone`].
pub fn date_in_zone(
    locale: &Locale,
    unix_seconds: i64,
    iana: &str,
    length: DateLength,
) -> Result<String, I18nError> {
    date_in_zone_with(
        locale,
        unix_seconds,
        iana,
        length,
        &DateTimeOptions::default(),
    )
}

/// [`date_in_zone`] with `options` overriding the calendar.
///
/// # Errors
///
/// As [`datetime_in_zone`].
pub fn date_in_zone_with(
    locale: &Locale,
    unix_seconds: i64,
    iana: &str,
    length: DateLength,
    options: &DateTimeOptions,
) -> Result<String, I18nError> {
    let (date, _) = zoned_civil(unix_seconds, iana)?;
    datetime::date_with(locale, date, length, options)
}

/// Formats the time of day the instant `unix_seconds` reads as in zone `iana`.
///
/// # Errors
///
/// As [`datetime_in_zone`].
pub fn time_in_zone(
    locale: &Locale,
    unix_seconds: i64,
    iana: &str,
    length: DateLength,
) -> Result<String, I18nError> {
    time_in_zone_with(
        locale,
        unix_seconds,
        iana,
        length,
        &DateTimeOptions::default(),
    )
}

/// [`time_in_zone`] with `options` overriding the hour cycle.
///
/// # Errors
///
/// As [`datetime_in_zone`].
pub fn time_in_zone_with(
    locale: &Locale,
    unix_seconds: i64,
    iana: &str,
    length: DateLength,
    options: &DateTimeOptions,
) -> Result<String, I18nError> {
    let (_, time) = zoned_civil(unix_seconds, iana)?;
    datetime::time_with(locale, time, length, options)
}

/// Formats the instant `unix_seconds` as a date-and-time at a fixed
/// `offset_seconds` east of UTC — for callers that carry an offset, not a
/// zone.
///
/// # Errors
///
/// [`civil_at_offset`]'s, plus [`datetime`](super::datetime)'s.
pub fn datetime_at_offset(
    locale: &Locale,
    unix_seconds: i64,
    offset_seconds: i32,
    length: DateLength,
) -> Result<String, I18nError> {
    datetime_at_offset_with(
        locale,
        unix_seconds,
        offset_seconds,
        length,
        &DateTimeOptions::default(),
    )
}

/// [`datetime_at_offset`] with `options` overriding the calendar and hour cycle.
///
/// # Errors
///
/// As [`datetime_at_offset`].
pub fn datetime_at_offset_with(
    locale: &Locale,
    unix_seconds: i64,
    offset_seconds: i32,
    length: DateLength,
    options: &DateTimeOptions,
) -> Result<String, I18nError> {
    let (date, time) = civil_at_offset(unix_seconds, offset_seconds)?;
    datetime::datetime_with(locale, date, time, length, options)
}

/// Parses a fixed UTC offset: `Z`, `±HH:MM`, `±HHMM` or `±HH`, to seconds
/// east of UTC. `None` for anything else, or an offset beyond ±23:59.
pub(super) fn parse_offset(text: &str) -> Option<i32> {
    if text == "Z" {
        return Some(0);
    }
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() == 4 && digits.is_ascii() => digits.split_at(2),
        None if digits.len() == 2 => (digits, "0"),
        None => return None,
    };
    if hours.len() != 2 || !(1..=2).contains(&minutes.len()) {
        return None;
    }
    if !hours
        .bytes()
        .chain(minutes.bytes())
        .all(|b| b.is_ascii_digit())
    {
        return None;
    }

    let (hours, minutes): (i32, i32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some(sign * (hours * 3_600 + minutes * 60))
}

/// Seconds since the Unix epoch of a UTC civil date-time, `None` when the
/// fields do not name a real date-time.
pub(super) fn utc_unix(date: CivilDate, time: CivilTime) -> Option<i64> {
    let civil = jiff::civil::DateTime::new(
        i16::try_from(date.year).ok()?,
        i8::try_from(date.month).ok()?,
        i8::try_from(date.day).ok()?,
        i8::try_from(time.hour).ok()?,
        i8::try_from(time.minute).ok()?,
        i8::try_from(time.second).ok()?,
        0,
    )
    .ok()?;

    Some(civil.to_zoned(TimeZone::UTC).ok()?.timestamp().as_second())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_offsets_parse_in_every_accepted_shape() {
        assert_eq!(parse_offset("Z"), Some(0));
        assert_eq!(parse_offset("+08:00"), Some(8 * 3_600));
        assert_eq!(parse_offset("-0530"), Some(-(5 * 3_600 + 30 * 60)));
        assert_eq!(parse_offset("+05"), Some(5 * 3_600));
        assert_eq!(parse_offset("+24:00"), None);
        assert_eq!(parse_offset("+08:60"), None);
        assert_eq!(parse_offset("08:00"), None);
        assert_eq!(parse_offset(""), None);
        assert_eq!(parse_offset("+8:00"), None);
    }

    #[test]
    fn an_unknown_zone_is_a_typed_error() {
        assert!(matches!(
            zone_offset_seconds(0, "Mars/Olympus"),
            Err(I18nError::UnknownTimeZone(zone)) if zone == "Mars/Olympus"
        ));
    }

    #[test]
    fn utc_unix_round_trips_a_known_instant() {
        let (date, time) = civil_at_offset(1_700_000_000, 0).expect("in range");
        assert_eq!(utc_unix(date, time), Some(1_700_000_000));
    }
}
