//! ICU4X-backed locale-aware formatting: numbers, percentages, currency,
//! dates, and times.
//!
//! Only compiled with the `formatting` feature (see `src/lib.rs`'s crate
//! doc) — independently toggleable from `frust-api`, so a headless build can
//! format with no facade dependency at all.
//!
//! ```
//! use frust_i18n::fmt::{self, CivilDate, DateLength};
//!
//! let de: frust_i18n::Locale = "de-DE".parse()?;
//! assert_eq!(fmt::decimal(&de, 1234.56), "1.234,56");
//! // U+00A0 between amount and symbol — CLDR's, not a plain space.
//! assert_eq!(fmt::currency(&de, 9.99, "EUR")?, "9,99\u{a0}€");
//!
//! let today = CivilDate { year: 2024, month: 1, day: 31 };
//! assert_eq!(fmt::date(&de, today, DateLength::Medium)?, "31.01.2024");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Two ways in, one implementation
//!
//! Everything here is reachable twice: directly, through the functions
//! below, and from inside an FTL message through `NUMBER()`/`DATETIME()`
//! once a [`LocaleSet`] has been through [`with_icu_functions`]. Both routes
//! run the same formatters over the same cache — see [`fluent_fns`] for the
//! FTL-side argument and option contract.
//!
//! # Data: baked CLDR, no provider to configure
//!
//! Every formatter is built from ICU4X's `compiled_data` (CLDR baked into
//! the binary), so there is no data provider, no async load, and no
//! locale-data file to ship. The flip side is that the data set is fixed at
//! compile time: the locale coverage is CLDR's, and there is no runtime way
//! to enumerate it.
//!
//! # Cost: construction is memoized, formatting is not blocking
//!
//! Building a formatter parses baked data and is comparatively expensive, so
//! each one is built once per (locale, kind) — plus length, where a kind has
//! one — and kept for the process's life. Formatting itself borrows a cached
//! formatter through an `Arc` clone and holds **no lock** while it runs, so a
//! formatting call never waits on another thread's ([`bridge`] documents the
//! contract). None of these functions performs IO, blocks, or allocates
//! beyond the string it returns: they are safe to call from a `build`/paint
//! path.
//!
//! # Degrading vs. failing
//!
//! [`decimal`] and [`percent`] return a `String`: a number that cannot be
//! formatted (non-finite, or a locale whose data failed to load) degrades to
//! Rust's own rendering with a warn-level log, because a UI showing an
//! un-localized number beats a UI showing nothing. Everything else returns
//! `Result<String, I18nError>`, since a bad currency code or an impossible
//! date is a caller error worth surfacing. Nothing here panics on any input.

mod bridge;
mod currency;
mod datetime;
mod fluent_fns;
mod number;

use crate::{I18nError, Locale, LocaleSet};

/// A calendar date with no time zone, in the proleptic Gregorian calendar
/// (ISO 8601) — the shape [`date`]/[`datetime`] take.
///
/// Rendered in the calendar CLDR gives the target locale, which is Gregorian
/// for all but a handful of locales; the fields themselves are always
/// proleptic Gregorian regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CivilDate {
    /// The proleptic-Gregorian year (1 BCE is `0`, 2 BCE is `-1`).
    pub year: i32,
    /// The month, 1–12.
    pub month: u8,
    /// The day of the month, 1–31.
    pub day: u8,
}

/// A wall-clock time with no time zone, to second precision — the shape
/// [`time`]/[`datetime`] take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CivilTime {
    /// The hour, 0–23, regardless of the locale's own hour cycle.
    pub hour: u8,
    /// The minute, 0–59.
    pub minute: u8,
    /// The second, 0–59.
    pub second: u8,
}

impl CivilTime {
    /// Midnight — the time a date-only input implies.
    pub const MIDNIGHT: Self = Self {
        hour: 0,
        minute: 0,
        second: 0,
    };
}

/// How much of a date/time to spell out.
///
/// Three shapes, mirroring the ones a `DateFormat` caller reaches for in
/// practice rather than the full UTS-35 skeleton vocabulary — see
/// [`datetime`](self::datetime)'s module doc for why the ladder is not the
/// extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateLength {
    /// Numeric and compact (`1/31/24`, `3:47 PM`).
    Short,
    /// Abbreviated names, seconds on a time (`Jan 31, 2024`, `3:47:50 PM`).
    Medium,
    /// Full names (`January 31, 2024`).
    Long,
}

/// Formats `value` with `locale`'s grouping and decimal separators.
///
/// The fraction is rounded to CLDR's standard three digits and never padded
/// — see [`number`](self::number)'s module doc. A value that cannot be
/// formatted degrades to Rust's own rendering with a log rather than
/// failing.
pub fn decimal(locale: &Locale, value: f64) -> String {
    number::decimal(locale, value)
}

/// Formats `value` as a percentage in `locale`'s percent pattern.
///
/// `value` is the **percentage**, not a ratio: `percent(&en, 75.0)` is
/// `"75%"`. (`Intl.NumberFormat`'s `style: "percent"` differs — it multiplies
/// by 100; this crate takes ICU4X's own convention, in both this function
/// and `NUMBER(style: "percent")`.)
///
/// Degrades like [`decimal`] rather than failing.
pub fn percent(locale: &Locale, value: f64) -> String {
    number::percent(locale, value)
}

/// Formats `amount` as the ISO 4217 currency `iso_code` in `locale`'s
/// currency pattern, rounded and padded to that currency's minor units
/// (`currency(&ja, 9.99, "JPY")` is `"￥10"`).
///
/// # Errors
///
/// [`I18nError::Format`] when `iso_code` is not three ASCII letters, when
/// `amount` is not finite, or when the locale's currency data fails to load.
/// A well-formed but unassigned code is *not* an error — see
/// [`currency`](self::currency)'s module doc.
pub fn currency(locale: &Locale, amount: f64, iso_code: &str) -> Result<String, I18nError> {
    currency::currency(locale, amount, iso_code)
}

/// Formats `date` at `length` in `locale`'s calendar.
///
/// # Errors
///
/// [`I18nError::Format`] when `date`'s fields do not name a real date, or
/// when the locale's date data fails to load.
pub fn date(locale: &Locale, date: CivilDate, length: DateLength) -> Result<String, I18nError> {
    datetime::date(locale, date, length)
}

/// Formats `time` at `length` in `locale`'s hour cycle (12-hour in `en-US`,
/// 24-hour in `de-DE`).
///
/// # Errors
///
/// [`I18nError::Format`] when `time`'s fields do not name a real time, or
/// when the locale's time data fails to load.
pub fn time(locale: &Locale, time: CivilTime, length: DateLength) -> Result<String, I18nError> {
    datetime::time(locale, time, length)
}

/// Formats `date` and `time` together at `length`, joined by `locale`'s own
/// glue pattern.
///
/// # Errors
///
/// The union of [`date`]'s and [`time`]'s.
pub fn datetime(
    locale: &Locale,
    date: CivilDate,
    time: CivilTime,
    length: DateLength,
) -> Result<String, I18nError> {
    datetime::datetime(locale, date, time, length)
}

/// Registers ICU-backed `NUMBER()` and `DATETIME()` into `set`, so FTL
/// messages can format through the same formatters as the functions above.
///
/// ```
/// # use frust_i18n::{Engine, LocaleSet, args, fmt};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = Engine::new(fmt::with_icu_functions(
///     LocaleSet::new("en-US".parse()?)
///         .with_locale("en-US".parse()?, [("app.ftl", "total = { NUMBER($n) }")])
///         .with_isolating(false),
/// ))?;
///
/// let chain = engine.negotiate(&["en-US".parse()?]);
/// assert_eq!(engine.resolve(&chain, "total", Some(&args!("n" => 1234.5)))?, "1,234.5");
/// # Ok(())
/// # }
/// ```
///
/// Call it last, after every `with_locale`/`with_function` call: registering
/// a `NUMBER` or `DATETIME` of your own as well makes [`Engine::new`](crate::Engine::new)
/// fail with a duplicate-function-id error. See [`fluent_fns`] for the
/// argument and option contract both functions honor.
pub fn with_icu_functions(set: LocaleSet) -> LocaleSet {
    fluent_fns::with_icu_functions(set)
}
