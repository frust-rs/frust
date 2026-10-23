//! Locale-aware decimal and percent formatting.
//!
//! # `f64` in, CLDR decimal pattern out
//!
//! Both entry points take an `f64` and route it through ICU4X's `Decimal`,
//! whose grouping separators, decimal separator, digit shapes, and minus sign
//! all come from the locale's CLDR data (`1,234.56` in `en-US`, `1.234,56` in
//! `de-DE`).
//!
//! The `f64` → `Decimal` crossing goes through Rust's own shortest-round-trip
//! `Display` (`Decimal::try_from_f64` needs `fixed_decimal`'s non-default
//! `ryu` feature, which nothing in this workspace's graph turns on). That is
//! exact for every value that reached the caller as decimal text, and gives
//! `0.1 + 0.2` the same 17 digits `println!("{x}")` would — which is why the
//! fraction is then rounded, below.
//!
//! # Options
//!
//! [`NumberOptions`] sets the fraction bounds, the grouping strategy and
//! compact notation; its `Default` reproduces the plain entry points. A
//! `min_fraction` pads (`4.8` → `4.80`) and a `max_fraction` rounds.
//!
//! # The one CLDR shape this applies itself: max three fraction digits
//!
//! CLDR's standard decimal pattern is `#,##0.###` — at most three fraction
//! digits — but ICU4X's `DecimalFormatter` deliberately renders a `Decimal`
//! exactly as handed to it and applies no pattern-driven digit bounds. So
//! this module rounds to three fraction digits itself (half-to-even, ICU's
//! own default mode) before formatting, matching what `Intl.NumberFormat`
//! and Dart's `NumberFormat.decimalPattern` produce for the same input. The
//! pattern's minimum fraction is zero digits, so trailing zeros come back off
//! afterwards: `1.5` stays `1.5`, never `1.500`.
//!
//! # Percent rides `icu_experimental`, like currency
//!
//! `PercentFormatter` gives the locale's real CLDR percent pattern — sign
//! and symbol placement included, which varies (`75%` vs `%75` vs `75 %`) —
//! rather than this module appending a `%` and guessing the side. The cost is
//! that percent shares [`currency`](super::currency)'s exposure to
//! `icu_experimental`: see that module's header for the migration contract.
//!
//! # Degradation
//!
//! Neither entry point is fallible to the caller ([`super::decimal`] and
//! [`super::percent`] return `String`). A non-finite input or a formatter
//! that fails to build logs and falls back to Rust's own rendering of the
//! number, so a UI shows an un-localized number rather than nothing.

use std::sync::LazyLock;

use icu_decimal::options::{
    CompactDecimalFormatterOptions, DecimalFormatterOptions, GroupingStrategy,
};
use icu_decimal::preferences::CompactDecimalFormatterPreferences;
use icu_decimal::{CompactDecimalFormatter, DecimalFormatter, DecimalFormatterPreferences};
use icu_experimental::dimension::percent::formatter::{
    PercentFormatter, PercentFormatterPreferences,
};

// `icu_decimal`'s own re-export of `fixed_decimal::Decimal` — reached
// through the component rather than the leaf crate, which is not a direct
// dependency here. The currency formatter takes the identical type.
use icu_decimal::input::Decimal;

use crate::Locale;

use super::bridge::{self, Cache};

/// CLDR's standard decimal pattern (`#,##0.###`) caps the fraction at three
/// digits.
const DEFAULT_MAX_FRACTION: u8 = 3;

/// The pattern's *minimum* fraction is zero digits (`#,##0.###` has no `0`
/// after the point), so trailing zeros come back off after rounding.
const DEFAULT_MIN_FRACTION: u8 = 0;

/// The most fraction digits an option may ask for; a larger request is
/// clamped rather than allowed to build an absurdly long string.
const FRACTION_CAP: u8 = 20;

/// When to render grouping separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Grouping {
    /// The locale's own rule (`1234` stays `1234` in `es`, `1,234` in `en`).
    #[default]
    Auto,
    /// Always group. ICU4X renders this identically to [`Grouping::Auto`] for
    /// a plain decimal; it differs only under compact notation, where `Auto`
    /// suppresses grouping below five digits.
    Always,
    /// Never render a grouping separator (`useGrouping: "false"`).
    Never,
}

/// How long a compact suffix is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompactStyle {
    /// `1.2K`.
    Short,
    /// `1.2 thousand`.
    Long,
}

/// The overall shape of the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Notation {
    /// Plain digits, honoring the fraction bounds.
    #[default]
    Standard,
    /// A scaled significand with the locale's magnitude word or suffix.
    ///
    /// ICU4X picks the significand's precision itself (one fraction digit
    /// below ten, none above), so the fraction bounds do not apply.
    Compact(CompactStyle),
}

/// Knobs [`decimal_with`](super::decimal_with) and
/// [`percent_with`](super::percent_with) accept. `Default` is exactly what
/// [`decimal`](super::decimal) and [`percent`](super::percent) do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NumberOptions {
    /// Fewest fraction digits; shorter fractions are zero-padded
    /// (`4.8` with `Some(2)` is `4.80`). Default zero.
    pub min_fraction: Option<u8>,
    /// Most fraction digits; longer fractions are rounded half-to-even.
    /// Default three, or `min_fraction` when that is larger.
    pub max_fraction: Option<u8>,
    /// When to render grouping separators.
    pub grouping: Grouping,
    /// Standard digits or compact notation. Applies to [`decimal_with`](super::decimal_with)
    /// only; a percent is always standard.
    pub notation: Notation,
}

impl NumberOptions {
    /// The `(min, max)` fraction digit counts after defaults and clamping.
    fn fraction_bounds(&self) -> (u8, u8) {
        let min = self.min_fraction.map(|min| min.min(FRACTION_CAP));
        let max = self.max_fraction.map(|max| max.min(FRACTION_CAP));
        match (min, max) {
            (None, None) => (DEFAULT_MIN_FRACTION, DEFAULT_MAX_FRACTION),
            (Some(min), None) => (min, min.max(DEFAULT_MAX_FRACTION)),
            (None, Some(max)) => (DEFAULT_MIN_FRACTION, max),
            (Some(min), Some(max)) => (min, min.max(max)),
        }
    }
}

fn strategy(grouping: Grouping) -> GroupingStrategy {
    match grouping {
        Grouping::Auto => GroupingStrategy::Auto,
        Grouping::Always => GroupingStrategy::Always,
        Grouping::Never => GroupingStrategy::Never,
    }
}

static DECIMALS: LazyLock<Cache<(Locale, Grouping), DecimalFormatter>> = LazyLock::new(Cache::new);
static PERCENTS: LazyLock<Cache<(Locale, Grouping), PercentFormatter<DecimalFormatter>>> =
    LazyLock::new(Cache::new);
static COMPACTS: LazyLock<Cache<(Locale, CompactStyle, Grouping), CompactDecimalFormatter>> =
    LazyLock::new(Cache::new);

/// Formats `value` with the locale's grouping and decimal separators.
pub(super) fn decimal(locale: &Locale, value: f64) -> String {
    decimal_with(locale, value, &NumberOptions::default())
}

/// [`decimal`] under `options`.
pub(super) fn decimal_with(locale: &Locale, value: f64, options: &NumberOptions) -> String {
    let Some(number) = to_decimal(value) else {
        return degrade(value, "not a finite number");
    };

    if let Notation::Compact(style) = options.notation {
        return compact(locale, value, number, style, options.grouping);
    }

    let (min, max) = options.fraction_bounds();
    match DECIMALS.get_or_build(&(locale.clone(), options.grouping), || {
        DecimalFormatter::try_new(
            DecimalFormatterPreferences::from(&bridge::lang_id(locale)),
            DecimalFormatterOptions::from(strategy(options.grouping)),
        )
    }) {
        Ok(formatter) => formatter
            .format(&with_fraction(number, min, max))
            .to_string(),
        Err(error) => degrade(value, &error.to_string()),
    }
}

/// The compact-notation arm of [`decimal_with`].
fn compact(
    locale: &Locale,
    value: f64,
    number: Decimal,
    style: CompactStyle,
    grouping: Grouping,
) -> String {
    let built = COMPACTS.get_or_build(&(locale.clone(), style, grouping), || {
        let preferences = CompactDecimalFormatterPreferences::from(&bridge::lang_id(locale));
        let options = CompactDecimalFormatterOptions::from(strategy(grouping));
        match style {
            CompactStyle::Short => CompactDecimalFormatter::try_new_short(preferences, options),
            CompactStyle::Long => CompactDecimalFormatter::try_new_long(preferences, options),
        }
    });
    match built {
        Ok(formatter) => formatter.format(&number).to_string(),
        Err(error) => degrade(value, &error.to_string()),
    }
}

/// Formats `value` as a percentage with the locale's percent pattern.
///
/// `value` is the percentage itself, not a ratio — see [`super::percent`].
pub(super) fn percent(locale: &Locale, value: f64) -> String {
    percent_with(locale, value, &NumberOptions::default())
}

/// [`percent`] under `options` (notation is ignored).
pub(super) fn percent_with(locale: &Locale, value: f64, options: &NumberOptions) -> String {
    let Some(number) = to_decimal(value) else {
        return degrade(value, "not a finite number");
    };

    let (min, max) = options.fraction_bounds();
    match PERCENTS.get_or_build(&(locale.clone(), options.grouping), || {
        let preferences = PercentFormatterPreferences::from(&bridge::lang_id(locale));
        let decimals = DecimalFormatter::try_new(
            (&preferences).into(),
            DecimalFormatterOptions::from(strategy(options.grouping)),
        )?;
        PercentFormatter::try_new_with_decimal_formatter(preferences, decimals, Default::default())
    }) {
        Ok(formatter) => formatter
            .format(&with_fraction(number, min, max))
            .to_string(),
        Err(error) => degrade(value, &error.to_string()),
    }
}

/// Applies CLDR's standard decimal fraction bounds: round at three digits,
/// then drop the trailing zeros rounding left behind.
pub(super) fn standard_fraction(number: Decimal) -> Decimal {
    with_fraction(number, DEFAULT_MIN_FRACTION, DEFAULT_MAX_FRACTION)
}

/// Rounds to `max` fraction digits, then pads (or trims zeros) to `min`.
///
/// `Decimal::round` sets the number's lower magnitude *to* the rounding
/// position, so it pads as well as rounds (`1.5` → `1.500`); `pad_end` is
/// what then sets the final width, and it never truncates a non-zero digit.
fn with_fraction(mut number: Decimal, min: u8, max: u8) -> Decimal {
    number.round(-i16::from(max));
    number.absolute.pad_end(-i16::from(min));
    number
}

/// Crosses an `f64` into ICU4X's `Decimal`, or `None` for a non-finite one.
///
/// Shared with [`currency`](super::currency), which applies its own
/// per-currency rounding to the result rather than this module's.
pub(super) fn to_decimal(value: f64) -> Option<Decimal> {
    if !value.is_finite() {
        return None;
    }
    // Rust's `f64: Display` never uses exponent notation, so this is always
    // plain decimal text `Decimal` can parse.
    value.to_string().parse::<Decimal>().ok()
}

/// The un-localized fallback rendering, logged with why it was reached.
fn degrade(value: f64, reason: &str) -> String {
    log::warn!("frust-i18n: formatting `{value}` fell back to plain digits: {reason}");
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    #[test]
    fn grouping_and_separators_follow_the_locale() {
        assert_eq!(decimal(&locale("en-US"), 1234.56), "1,234.56");
        assert_eq!(decimal(&locale("de-DE"), 1234.56), "1.234,56");
        assert_eq!(decimal(&locale("ja-JP"), 1234.56), "1,234.56");
    }

    #[test]
    fn a_negative_number_uses_the_locales_minus_sign() {
        assert_eq!(decimal(&locale("en-US"), -1234.5), "-1,234.5");
    }

    #[test]
    fn binary_float_noise_is_rounded_off_at_three_fraction_digits() {
        assert_eq!(decimal(&locale("en-US"), 0.1 + 0.2), "0.3");
        assert_eq!(decimal(&locale("en-US"), 1.23456), "1.235");
    }

    #[test]
    fn trailing_zeros_are_never_padded_on() {
        assert_eq!(decimal(&locale("en-US"), 1.5), "1.5");
        assert_eq!(decimal(&locale("en-US"), 2.0), "2");
    }

    #[test]
    fn a_non_finite_number_degrades_instead_of_panicking() {
        assert_eq!(decimal(&locale("en-US"), f64::NAN), "NaN");
        assert_eq!(decimal(&locale("en-US"), f64::INFINITY), "inf");
    }

    #[test]
    fn percent_carries_the_locales_own_symbol_placement() {
        // The value is the percentage, not a ratio.
        let en = percent(&locale("en-US"), 75.0);
        let de = percent(&locale("de-DE"), 75.0);

        assert!(en.contains('%') && en.contains("75"), "{en}");
        assert!(de.contains('%') && de.contains("75"), "{de}");
    }

    #[test]
    fn percent_groups_its_digits_like_a_decimal() {
        assert_eq!(percent(&locale("en-US"), 12345.67), "12,345.67%");
    }
}
