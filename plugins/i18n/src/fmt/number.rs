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

use icu_decimal::options::DecimalFormatterOptions;
use icu_decimal::{DecimalFormatter, DecimalFormatterPreferences};
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
/// digits; ICU4X positions are powers of ten, so that cap is position `-3`.
const MAX_FRACTION_POSITION: i16 = -3;

/// The pattern's *minimum* fraction is zero digits (`#,##0.###` has no `0`
/// after the point), so trailing zeros come back off after rounding.
const MIN_FRACTION_POSITION: i16 = 0;

static DECIMALS: LazyLock<Cache<Locale, DecimalFormatter>> = LazyLock::new(Cache::new);
static PERCENTS: LazyLock<Cache<Locale, PercentFormatter<DecimalFormatter>>> =
    LazyLock::new(Cache::new);

/// Formats `value` with the locale's grouping and decimal separators.
pub(super) fn decimal(locale: &Locale, value: f64) -> String {
    let Some(number) = to_decimal(value) else {
        return degrade(value, "not a finite number");
    };

    match DECIMALS.get_or_build(locale, || {
        DecimalFormatter::try_new(
            DecimalFormatterPreferences::from(&bridge::lang_id(locale)),
            DecimalFormatterOptions::default(),
        )
    }) {
        Ok(formatter) => formatter.format(&standard_fraction(number)).to_string(),
        Err(error) => degrade(value, &error.to_string()),
    }
}

/// Formats `value` as a percentage with the locale's percent pattern.
///
/// `value` is the percentage itself, not a ratio — see [`super::percent`].
pub(super) fn percent(locale: &Locale, value: f64) -> String {
    let Some(number) = to_decimal(value) else {
        return degrade(value, "not a finite number");
    };

    match PERCENTS.get_or_build(locale, || {
        PercentFormatter::try_new(
            PercentFormatterPreferences::from(&bridge::lang_id(locale)),
            Default::default(),
        )
    }) {
        Ok(formatter) => formatter.format(&standard_fraction(number)).to_string(),
        Err(error) => degrade(value, &error.to_string()),
    }
}

/// Applies CLDR's standard decimal fraction bounds: round at three digits,
/// then drop the trailing zeros rounding left behind.
///
/// `Decimal::round` sets the number's lower magnitude *to* the rounding
/// position, so it pads as well as rounds (`1.5` → `1.500`); `pad_end` is
/// what takes the padding back off, and it never truncates a non-zero digit.
fn standard_fraction(mut number: Decimal) -> Decimal {
    number.round(MAX_FRACTION_POSITION);
    number.absolute.pad_end(MIN_FRACTION_POSITION);
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
