//! `NUMBER()` and `DATETIME()` for FTL messages, over the same formatters
//! the direct API uses.
//!
//! # Registration
//!
//! [`super::with_icu_functions`] installs both through
//! [`LocaleSet::with_locale_function`], so each locale's bundle gets a
//! closure holding that locale — `fluent-bundle`'s own function seam is
//! locale-agnostic, and a formatter cannot be chosen without the locale. The
//! engine deliberately never calls `add_builtins()`, so the `NUMBER` id is
//! free rather than already taken by Fluent's own (non-ICU) implementation.
//!
//! # `NUMBER($n, …)`
//!
//! Positional argument: a number, or a string that parses as one. Honored
//! named options:
//!
//! | Option | Values | Effect |
//! |---|---|---|
//! | `style` | `decimal` (default), `percent`, `currency` | which formatter runs |
//! | `currency` | an ISO 4217 code | required by `style: "currency"` |
//!
//! `style: "percent"` takes the **percentage**, not a ratio: `NUMBER(75,
//! style: "percent")` is `75%`. This follows ICU4X's own `PercentFormatter`
//! and [`super::percent`], and deliberately differs from ECMA-402's
//! `Intl.NumberFormat`, which multiplies its input by 100. One rule across
//! the direct API and FTL was worth more than matching JavaScript.
//!
//! # `DATETIME($d, …)`
//!
//! Positional argument: either seconds since the Unix epoch (a number,
//! resolved in **UTC**) or an ISO-8601 string — `YYYY-MM-DD`,
//! `YYYY-MM-DDTHH:MM`, or `YYYY-MM-DDTHH:MM:SS`, with an optional trailing
//! `Z`. A `T` may be a space. No offset other than `Z` is accepted: this
//! crate has no time-zone conversion (see [`datetime`](super::datetime)).
//! Honored named options:
//!
//! | Option | Values | Effect |
//! |---|---|---|
//! | `dateStyle` | `short`, `medium` (default), `long` | the date half's length |
//! | `timeStyle` | `short`, `medium`, `long` | adds the time half |
//!
//! With neither, a medium date is formatted. With `timeStyle` alone, only the
//! time. With both, the combined form at `dateStyle`'s length.
//!
//! # Everything else is ignored, at `debug` level
//!
//! Any other named option — ECMA-402's `minimumFractionDigits`,
//! `currencyDisplay`, `useGrouping`, … — is dropped with a `debug!` log, not
//! a warning and never an error: these functions run on a UI's formatting
//! path, potentially per frame, so an unhonored option must not be able to
//! flood a release log.
//!
//! # A bad argument renders Fluent's placeholder, it does not fail the message
//!
//! An unusable positional argument (`DATETIME("yesterday")`, a
//! `style: "currency"` with no `currency:`) resolves to `FluentValue::Error`
//! plus a `debug!` log naming the cause. `fluent-bundle` renders that as the
//! call's own source text — the message formats as `DATETIME()` — and
//! deliberately records **no** resolver error, so [`Engine::resolve`](crate::Engine::resolve)
//! returns `Ok` with the placeholder in place rather than
//! [`I18nError`](crate::I18nError). That is `fluent-bundle`'s contract for
//! every failed function call, not a choice this module can make
//! differently; the log is the diagnostic, and a visible `DATETIME()` in the
//! UI is the tell.

use std::sync::Arc;

use fluent_bundle::{FluentArgs, FluentValue};

use crate::{FluentFunction, Locale, LocaleSet};

use super::{CivilDate, CivilTime, DateLength, datetime};

/// Registers ICU-backed `NUMBER` and `DATETIME` into `set`.
pub(super) fn with_icu_functions(set: LocaleSet) -> LocaleSet {
    set.with_locale_function("NUMBER", |locale| {
        let locale = locale.clone();
        let function: FluentFunction =
            Arc::new(move |positional, named| number(&locale, positional, named));
        function
    })
    .with_locale_function("DATETIME", |locale| {
        let locale = locale.clone();
        let function: FluentFunction =
            Arc::new(move |positional, named| date_time(&locale, positional, named));
        function
    })
}

/// `NUMBER($n, style: …, currency: …)`.
fn number<'a>(
    locale: &Locale,
    positional: &[FluentValue<'a>],
    named: &FluentArgs<'_>,
) -> FluentValue<'a> {
    let Some(value) = as_f64(positional.first()) else {
        return FluentValue::Error;
    };

    let formatted = match option(named, "style").unwrap_or("decimal") {
        "decimal" => super::decimal(locale, value),
        "percent" => super::percent(locale, value),
        "currency" => {
            let Some(code) = option(named, "currency") else {
                log::debug!("frust-i18n: NUMBER(style: \"currency\") without a `currency` option");
                return FluentValue::Error;
            };
            match super::currency(locale, value, code) {
                Ok(formatted) => formatted,
                Err(error) => {
                    log::debug!("frust-i18n: NUMBER(style: \"currency\"): {error}");
                    return FluentValue::Error;
                }
            }
        }
        unknown => {
            log::debug!("frust-i18n: NUMBER(style: \"{unknown}\") — formatting as a decimal");
            super::decimal(locale, value)
        }
    };

    report_unhonored(named, "NUMBER", &["style", "currency"]);
    FluentValue::from(formatted)
}

/// `DATETIME($d, dateStyle: …, timeStyle: …)`.
fn date_time<'a>(
    locale: &Locale,
    positional: &[FluentValue<'a>],
    named: &FluentArgs<'_>,
) -> FluentValue<'a> {
    let Some((date, time)) = as_civil(positional.first()) else {
        return FluentValue::Error;
    };

    let date_style = option(named, "dateStyle").map(length);
    let time_style = option(named, "timeStyle").map(length);

    let formatted = match (date_style, time_style) {
        (Some(date_style), Some(_)) => datetime::datetime(locale, date, time, date_style),
        (None, Some(time_style)) => datetime::time(locale, time, time_style),
        (date_style, None) => {
            datetime::date(locale, date, date_style.unwrap_or(DateLength::Medium))
        }
    };

    report_unhonored(named, "DATETIME", &["dateStyle", "timeStyle"]);
    match formatted {
        Ok(formatted) => FluentValue::from(formatted),
        Err(error) => {
            log::debug!("frust-i18n: DATETIME(): {error}");
            FluentValue::Error
        }
    }
}

/// Reads a named option as a string, ignoring a non-string value.
///
/// Scanned rather than looked up: `FluentArgs::get` ties its key's lifetime
/// to the argument set's, which a `&'static` option name here cannot satisfy.
/// An options bag holds a handful of entries, so the scan is not the cost.
fn option<'a>(named: &'a FluentArgs<'_>, key: &str) -> Option<&'a str> {
    named.iter().find_map(|(name, value)| match value {
        FluentValue::String(text) if name == key => Some(text.as_ref()),
        _ => None,
    })
}

/// Maps a `dateStyle`/`timeStyle` value onto a length, defaulting to medium.
fn length(style: &str) -> DateLength {
    match style {
        "short" => DateLength::Short,
        "long" => DateLength::Long,
        "medium" => DateLength::Medium,
        unknown => {
            log::debug!("frust-i18n: DATETIME(…: \"{unknown}\") — using the medium length");
            DateLength::Medium
        }
    }
}

/// Logs every named option outside `honored`.
fn report_unhonored(named: &FluentArgs<'_>, function: &str, honored: &[&str]) {
    for (key, _) in named.iter() {
        if !honored.contains(&key) {
            log::debug!("frust-i18n: {function}() ignores the `{key}` option");
        }
    }
}

/// Reads a positional argument as a number.
fn as_f64(value: Option<&FluentValue<'_>>) -> Option<f64> {
    match value {
        Some(FluentValue::Number(number)) => Some(number.value),
        Some(FluentValue::String(text)) => text.parse().ok(),
        _ => None,
    }
}

/// Reads a positional argument as a civil date-time — epoch seconds, or an
/// ISO-8601 string.
fn as_civil(value: Option<&FluentValue<'_>>) -> Option<(CivilDate, CivilTime)> {
    match value {
        Some(FluentValue::Number(number)) => {
            datetime::from_unix_seconds(number.value.trunc() as i64)
        }
        Some(FluentValue::String(text)) => parse_iso(text),
        _ => None,
    }
}

/// Parses the ISO-8601 subset this crate accepts (see the module header).
fn parse_iso(text: &str) -> Option<(CivilDate, CivilTime)> {
    let text = text.strip_suffix('Z').unwrap_or(text);
    let (date, time) = match text.split_once(['T', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };

    let mut date_parts = date.split('-');
    let date = CivilDate {
        year: date_parts.next()?.parse().ok()?,
        month: date_parts.next()?.parse().ok()?,
        day: date_parts.next()?.parse().ok()?,
    };
    if date_parts.next().is_some() {
        return None;
    }

    let Some(time) = time else {
        return Some((date, CivilTime::MIDNIGHT));
    };
    let mut time_parts = time.split(':');
    let time = CivilTime {
        hour: time_parts.next()?.parse().ok()?,
        minute: time_parts.next()?.parse().ok()?,
        second: match time_parts.next() {
            Some(second) => second.parse().ok()?,
            None => 0,
        },
    };
    if time_parts.next().is_some() {
        return None;
    }

    Some((date, time))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn engine(ftl: &'static str) -> Engine {
        Engine::new(with_icu_functions(
            LocaleSet::new(locale("en-US"))
                .with_locale(locale("en-US"), [("t.ftl", ftl)])
                .with_locale(locale("de-DE"), [("t.ftl", ftl)])
                .with_isolating(false),
        ))
        .expect("engine builds")
    }

    fn resolved(ftl: &'static str, tag: &str, args: fluent_bundle::FluentArgs<'_>) -> String {
        engine(ftl)
            .resolve(&[locale(tag)], "m", Some(&args))
            .expect("resolves")
    }

    #[test]
    fn number_formats_through_icu_per_locale() {
        let ftl = "m = { NUMBER($n) }";

        assert_eq!(
            resolved(ftl, "en-US", crate::args!("n" => 1234.56)),
            "1,234.56"
        );
        assert_eq!(
            resolved(ftl, "de-DE", crate::args!("n" => 1234.56)),
            "1.234,56"
        );
    }

    #[test]
    fn number_honors_the_percent_and_currency_styles() {
        let percent = resolved(
            r#"m = { NUMBER($n, style: "percent") }"#,
            "en-US",
            crate::args!("n" => 75),
        );
        let currency = resolved(
            r#"m = { NUMBER($n, style: "currency", currency: "USD") }"#,
            "en-US",
            crate::args!("n" => 9.99),
        );

        assert_eq!(percent, "75%");
        assert_eq!(currency, "$9.99");
    }

    #[test]
    fn an_unhonored_option_is_ignored_rather_than_failing() {
        let formatted = resolved(
            r#"m = { NUMBER($n, useGrouping: "false") }"#,
            "en-US",
            crate::args!("n" => 1234),
        );

        assert_eq!(formatted, "1,234");
    }

    #[test]
    fn a_currency_style_without_a_code_renders_fluents_placeholder() {
        let rendered = resolved(
            r#"m = { NUMBER($n, style: "currency") }"#,
            "en-US",
            crate::args!("n" => 1),
        );

        assert_eq!(rendered, "NUMBER()");
    }

    #[test]
    fn datetime_formats_epoch_seconds_and_iso_strings_alike() {
        let ftl = "m = { DATETIME($d) }";
        let from_epoch = resolved(ftl, "en-US", crate::args!("d" => 1_706_659_200_i64));
        let from_string = resolved(ftl, "en-US", crate::args!("d" => "2024-01-31"));

        assert_eq!(from_epoch, from_string);
        assert!(from_epoch.contains("2024"), "{from_epoch}");
    }

    #[test]
    fn datetime_styles_select_the_halves() {
        let date_only = resolved(
            r#"m = { DATETIME($d, dateStyle: "short") }"#,
            "en-US",
            crate::args!("d" => "2024-01-31T15:47:50"),
        );
        let time_only = resolved(
            r#"m = { DATETIME($d, timeStyle: "short") }"#,
            "en-US",
            crate::args!("d" => "2024-01-31T15:47:50"),
        );
        let both = resolved(
            r#"m = { DATETIME($d, dateStyle: "medium", timeStyle: "short") }"#,
            "en-US",
            crate::args!("d" => "2024-01-31T15:47:50"),
        );

        assert!(!date_only.contains("47"), "{date_only}");
        assert!(!time_only.contains("31"), "{time_only}");
        assert!(both.contains("2024") && both.contains("47"), "{both}");
    }

    #[test]
    fn a_malformed_datetime_argument_renders_fluents_placeholder() {
        let rendered = resolved(
            "m = { DATETIME($d) }",
            "en-US",
            crate::args!("d" => "not a date"),
        );

        assert_eq!(rendered, "DATETIME()");
    }

    #[test]
    fn the_iso_subset_covers_dates_datetimes_and_a_trailing_zulu() {
        let midnight = CivilTime::MIDNIGHT;

        assert_eq!(
            parse_iso("2024-01-31"),
            Some((
                CivilDate {
                    year: 2024,
                    month: 1,
                    day: 31
                },
                midnight
            ))
        );
        assert_eq!(
            parse_iso("2024-01-31T15:47:50Z").map(|(_, time)| time),
            Some(CivilTime {
                hour: 15,
                minute: 47,
                second: 50
            })
        );
        assert_eq!(
            parse_iso("2024-01-31 15:47").map(|(_, time)| time),
            Some(CivilTime {
                hour: 15,
                minute: 47,
                second: 0
            })
        );
        assert_eq!(parse_iso("2024-01"), None);
        assert_eq!(parse_iso("2024-01-31T15:47:50+02:00"), None);
    }
}
