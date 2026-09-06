//! Locale-aware currency formatting.
//!
//! # This file is the whole `icu_experimental` currency surface, on purpose
//!
//! ICU4X has no graduated currency component yet: `CurrencyFormatter` lives
//! in `icu_experimental`, whose own crate doc says "all code in this crate is
//! unstable". Everything this crate knows about that type is therefore
//! confined here — [`super::currency`] is the only caller, and it speaks
//! `&Locale`/`f64`/`&str`/`String`. When the component graduates, this file
//! changes and nothing else does. Do not widen its signature to pass an
//! `icu_experimental` type outward.
//!
//! (Percent formatting rides the same crate from
//! [`number`](super::number) — the exception, and the second file to review
//! on a graduation.)
//!
//! # Minor units are applied here, not by ICU4X
//!
//! `CurrencyFormatter::format_fixed_decimal` renders the `Decimal` it is
//! handed exactly, and ICU4X 2.2.x exposes no CLDR `currencyData` minor-unit
//! digits at all. So this module rounds and pads to the currency's own digit
//! count itself: two digits for almost every currency, zero for the
//! [`ZERO_DIGIT`] set (`JPY 9.99` → `¥10`), three for the [`THREE_DIGIT`]
//! set. Both lists are the *active* currencies of CLDR's
//! `supplemental/currencyData.json` `fractions` block — a documented
//! simplification: historic currencies with non-default digit counts (ADP,
//! ITL, TRL, …) fall to the two-digit default rather than being carried.
//!
//! # What "unknown currency code" means
//!
//! A code that is not three ASCII letters is a typed
//! [`I18nError::Format`] — the caller passed something that is not an ISO
//! 4217 code at all. A *well-formed but unassigned* code (`ZZZ`) is **not**
//! an error: CLDR's own fallback renders such a code as its own symbol, and
//! rejecting it would mean shipping and maintaining a full ISO 4217 register
//! this crate has no source for. Callers that need registry-level validation
//! do it before calling.

use std::sync::LazyLock;

use icu_experimental::dimension::currency::CurrencyCode;
use icu_experimental::dimension::currency::formatter::{
    CurrencyFormatter, CurrencyFormatterPreferences,
};
use tinystr::TinyAsciiStr;

use crate::{I18nError, Locale};

use super::bridge::{self, Cache};
use super::number;

/// The number of ASCII letters in an ISO 4217 code.
const CODE_LEN: usize = 3;

/// Currencies CLDR gives zero minor-unit digits (`fractions`'s `digits="0"`),
/// restricted to codes in active use.
const ZERO_DIGIT: [&str; 17] = [
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "UYI", "VND",
    "VUV", "XAF", "XOF", "XPF",
];

/// Currencies CLDR gives three minor-unit digits (`fractions`'s `digits="3"`),
/// restricted to codes in active use.
const THREE_DIGIT: [&str; 7] = ["BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND"];

/// CLDR's default when a currency is absent from the `fractions` block.
const DEFAULT_DIGITS: i16 = 2;

static CURRENCIES: LazyLock<Cache<Locale, CurrencyFormatter>> = LazyLock::new(Cache::new);

/// Formats `amount` as `iso_code` in `locale`'s currency pattern.
///
/// # Errors
///
/// [`I18nError::Format`] for a code that is not three ASCII letters, for a
/// non-finite `amount`, or when the locale's currency data fails to load.
pub(super) fn currency(locale: &Locale, amount: f64, iso_code: &str) -> Result<String, I18nError> {
    let code = parse_code(iso_code)?;

    let mut value = number::to_decimal(amount)
        .ok_or_else(|| I18nError::Format(format!("`{amount}` is not a finite currency amount")))?;
    let position = -fraction_digits(iso_code);
    value.round(position);
    // Padding is on the unsigned half; `Decimal` carries its sign separately.
    value.absolute.pad_end(position);

    let formatter = CURRENCIES
        .get_or_build(locale, || {
            CurrencyFormatter::try_new(
                CurrencyFormatterPreferences::from(&bridge::lang_id(locale)),
                Default::default(),
            )
        })
        .map_err(|error| I18nError::Format(format!("{locale}: loading currency data: {error}")))?;

    Ok(formatter.format_fixed_decimal(&value, &code).to_string())
}

/// Validates and normalizes an ISO 4217 code into ICU4X's own newtype.
fn parse_code(iso_code: &str) -> Result<CurrencyCode, I18nError> {
    let malformed = || {
        I18nError::Format(format!(
            "`{iso_code}` is not an ISO 4217 currency code (expected three ASCII letters)"
        ))
    };

    if iso_code.len() != CODE_LEN || !iso_code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(malformed());
    }

    TinyAsciiStr::<CODE_LEN>::try_from_str(iso_code)
        .map(|code| CurrencyCode(code.to_ascii_uppercase()))
        .map_err(|_| malformed())
}

/// The currency's CLDR minor-unit digit count.
fn fraction_digits(iso_code: &str) -> i16 {
    let upper = iso_code.to_ascii_uppercase();
    if ZERO_DIGIT.contains(&upper.as_str()) {
        return 0;
    }
    if THREE_DIGIT.contains(&upper.as_str()) {
        return 3;
    }
    DEFAULT_DIGITS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn format(tag: &str, amount: f64, code: &str) -> String {
        currency(&locale(tag), amount, code).expect("formats")
    }

    #[test]
    fn the_symbol_and_its_placement_follow_the_locale() {
        let en = format("en-US", 9.99, "USD");
        let de = format("de-DE", 9.99, "EUR");
        let fr = format("fr-FR", 9.99, "EUR");

        assert!(en.starts_with('$'), "{en}");
        assert!(de.ends_with('€'), "{de}");
        assert!(fr.ends_with('€'), "{fr}");
        assert!(de.contains("9,99"), "{de}");
    }

    #[test]
    fn a_zero_digit_currency_drops_its_fraction() {
        let jpy = format("ja-JP", 9.99, "JPY");

        assert!(jpy.contains("10"), "{jpy}");
        assert!(!jpy.contains('.'), "{jpy}");
    }

    #[test]
    fn a_two_digit_currency_pads_to_two() {
        assert!(format("en-US", 9.0, "USD").contains("9.00"));
    }

    #[test]
    fn a_three_digit_currency_pads_to_three() {
        assert!(format("en-US", 9.5, "KWD").contains("9.500"));
    }

    #[test]
    fn a_lowercase_code_is_normalized() {
        assert_eq!(format("en-US", 1.0, "usd"), format("en-US", 1.0, "USD"));
    }

    #[test]
    fn a_malformed_code_is_a_typed_error() {
        for code in ["", "US", "USDX", "US1", "€€€"] {
            let error = currency(&locale("en-US"), 1.0, code).expect_err("rejects `{code}`");
            assert!(matches!(error, I18nError::Format(_)), "{code}: {error:?}");
        }
    }

    #[test]
    fn a_non_finite_amount_is_a_typed_error() {
        let error = currency(&locale("en-US"), f64::NAN, "USD").expect_err("rejects NaN");

        assert!(matches!(error, I18nError::Format(_)), "{error:?}");
    }

    #[test]
    fn a_well_formed_unassigned_code_renders_rather_than_failing() {
        let rendered = format("en-US", 1.0, "ZZZ");

        assert!(rendered.contains("ZZZ"), "{rendered}");
    }
}
