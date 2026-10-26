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
//! # One digit source: the baked CLDR fraction data
//!
//! `icu_experimental` 0.7's formatter re-rounds every value to the digits of
//! its own baked `CurrencyFractionsV1` data (`apply_precision`), whatever the
//! caller passed. A digit list of this crate's own would therefore be a second
//! opinion that could only ever be wrong: it would pad `IQD` to three digits
//! that the formatter then rounds back off, and leave `COP` at two digits that
//! it rounds to zero. So this module asks the *same* data —
//! `CurrencyFractions::resolve` over the baked provider — and pads to exactly
//! what the formatter keeps (`JPY 9.99` → `¥10`, `KWD 9.5` → `9.500`). There is
//! no list to maintain and a data update moves both sides together. A code the
//! data does not list takes the data's own default (two digits). The test
//! `minor_unit_digits_equal_what_the_formatter_renders` walks every code in the
//! data to hold the two together.
//!
//! The formatter's `Rounding` increments (`R5`/`R20`/`R50`) are not modelled;
//! CLDR 49's standard (non-cash) rounding is `R1` for every currency, which the
//! same test asserts, so a data update that introduces one fails loudly rather
//! than rendering differently.
//!
//! # The formatter cache is bounded
//!
//! Formatters are cached per `(locale, currency)` in a [`BoundedCache`] of at
//! most [`CACHE_CAPACITY`] entries, least-recently-used evicted first, so a
//! caller cycling through arbitrary locale/currency pairs cannot grow it
//! without limit. An evicted pair is simply rebuilt on its next use.
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

use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use icu_decimal::DecimalFormatter;
use icu_decimal::input::Decimal;
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::currency::formatter::{
    CurrencyFormatter, CurrencyFormatterPreferences,
};
use icu_experimental::dimension::provider::currency::fractions::CurrencyFractionsV1;
use icu_provider::{DataPayload, DataProvider, DataRequest};
use tinystr::TinyAsciiStr;

use crate::{I18nError, Locale};

use super::bridge;
use super::number;

/// The number of ASCII letters in an ISO 4217 code.
const CODE_LEN: usize = 3;

/// CLDR's default when the data cannot be loaded at all (it is baked in, so
/// this is unreachable in practice).
const DEFAULT_DIGITS: i16 = 2;

/// The most `(locale, currency)` formatters kept alive at once.
const CACHE_CAPACITY: usize = 64;

/// The baked CLDR fraction data, loaded once.
static FRACTIONS: LazyLock<Option<DataPayload<CurrencyFractionsV1>>> = LazyLock::new(|| {
    DataProvider::<CurrencyFractionsV1>::load(&icu_experimental::provider::Baked, {
        DataRequest::default()
    })
    .ok()
    .map(|response| response.payload)
});

type Formatter = CurrencyFormatter<DecimalFormatter>;

static CURRENCIES: LazyLock<BoundedCache<(Locale, CurrencyType), Formatter>> =
    LazyLock::new(|| BoundedCache::new(CACHE_CAPACITY));

/// A least-recently-used cache holding at most `capacity` entries.
///
/// The lock covers only the lookup and the insert, never a build or a format;
/// two threads racing a cold key both build and one insert wins, which is
/// wasted work and never a wrong answer.
struct BoundedCache<K, V> {
    capacity: usize,
    /// Most recently used last.
    entries: Mutex<Vec<(K, Arc<V>)>>,
}

impl<K: Clone + Eq, V> BoundedCache<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Mutex::new(Vec::new()),
        }
    }

    fn get_or_build<E>(&self, key: &K, build: impl FnOnce() -> Result<V, E>) -> Result<Arc<V>, E> {
        {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(index) = entries.iter().position(|(held, _)| held == key) {
                let hit = entries.remove(index);
                let found = Arc::clone(&hit.1);
                entries.push(hit);
                return Ok(found);
            }
        }

        let built = Arc::new(build()?);
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = entries.iter().position(|(held, _)| held == key) {
            entries.remove(index);
        }
        entries.push((key.clone(), Arc::clone(&built)));
        if entries.len() > self.capacity {
            entries.remove(0);
        }
        Ok(built)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

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
    let position = -fraction_digits(&code);
    // No rounding here: the formatter rounds half-expand to the same CLDR digits,
    // and a second rounding mode of ours would only disagree on ties. Padding is
    // on the unsigned half; `Decimal` carries its sign separately.
    value.absolute.pad_end(position);

    render(locale, &value, &code)
}

/// The currency's CLDR minor-unit digit count (`JPY` 0, `USD` 2, `KWD` 3).
///
/// # Errors
///
/// [`I18nError::Format`] for a code that is not three ASCII letters.
pub(super) fn minor_unit_digits(iso_code: &str) -> Result<u8, I18nError> {
    let code = parse_code(iso_code)?;
    Ok(fraction_digits(&code) as u8)
}

/// Formats `minor` (an integer count of the currency's minor units) exactly:
/// the `Decimal` is built from the integer and shifted, never via `f64`.
pub(super) fn currency_minor(
    locale: &Locale,
    minor: i64,
    iso_code: &str,
) -> Result<String, I18nError> {
    let code = parse_code(iso_code)?;
    let digits = -fraction_digits(&code);

    let mut value = Decimal::from(minor);
    value.multiply_pow10(digits);
    value.absolute.pad_end(digits);

    render(locale, &value, &code)
}

/// Runs the cached locale formatter over an already-rounded `value`.
fn render(locale: &Locale, value: &Decimal, code: &CurrencyType) -> Result<String, I18nError> {
    let formatter = CURRENCIES
        .get_or_build(&(locale.clone(), *code), || {
            CurrencyFormatter::try_new_symbol(
                CurrencyFormatterPreferences::from(&bridge::lang_id(locale)),
                *code,
                Default::default(),
            )
        })
        .map_err(|error| I18nError::Format(format!("{locale}: loading currency data: {error}")))?;

    Ok(formatter.format_fixed_decimal(value).to_string())
}

/// Validates and normalizes an ISO 4217 code into ICU4X's own keyword type.
fn parse_code(iso_code: &str) -> Result<CurrencyType, I18nError> {
    let malformed = || {
        I18nError::Format(format!(
            "`{iso_code}` is not an ISO 4217 currency code (expected three ASCII letters)"
        ))
    };

    if iso_code.len() != CODE_LEN || !iso_code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(malformed());
    }

    TinyAsciiStr::<CODE_LEN>::try_from_str(iso_code)
        .ok()
        .and_then(|code| CurrencyType::try_from_str(code.to_ascii_uppercase().as_str()).ok())
        .ok_or_else(malformed)
}

/// The digits the formatter keeps for `code`: its baked CLDR fraction data,
/// the data's default for an unlisted code.
fn fraction_digits(code: &CurrencyType) -> i16 {
    FRACTIONS.as_ref().map_or(DEFAULT_DIGITS, |payload| {
        i16::from(payload.get().resolve(*code).digits)
    })
}

#[cfg(test)]
mod tests {
    use icu_experimental::dimension::provider::currency::fractions::{CurrencyFractions, Rounding};

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
    fn minor_unit_digits_reads_the_tables_and_rejects_malformed_codes() {
        assert_eq!(minor_unit_digits("JPY").unwrap(), 0);
        assert_eq!(minor_unit_digits("usd").unwrap(), 2);
        assert_eq!(minor_unit_digits("KWD").unwrap(), 3);
        assert_eq!(minor_unit_digits("IQD").unwrap(), 0);
        assert_eq!(minor_unit_digits("COP").unwrap(), 0);
        assert!(minor_unit_digits("US").is_err());
    }

    /// Fraction digits the formatter renders for `code`, read off the output.
    fn rendered_fraction_digits(code: &str) -> usize {
        let text = format("en-US", 1234.5678, code);
        let after = text
            .split_once("1,23")
            .map(|(_, rest)| &rest[1..])
            .unwrap_or_else(|| panic!("{code}: {text}"));
        match after.strip_prefix('.') {
            Some(fraction) => fraction.chars().take_while(char::is_ascii_digit).count(),
            None => 0,
        }
    }

    #[test]
    fn minor_unit_digits_equal_what_the_formatter_renders() {
        let payload = FRACTIONS.as_ref().expect("baked fraction data loads");
        let data: &CurrencyFractions<'_> = payload.get();
        assert_eq!(data.default.rounding, Rounding::R1);

        let mut checked = 0;
        for key in data.fractions.iter_keys() {
            let code = key.try_into_tinystr().expect("valid code");
            let code = code.as_str();
            let info = data.resolve(parse_code(code).expect("data code parses"));
            assert_eq!(info.rounding, Rounding::R1, "{code}");
            assert_eq!(
                usize::from(minor_unit_digits(code).unwrap()),
                usize::from(info.digits),
                "{code} digits"
            );
            assert_eq!(
                rendered_fraction_digits(code),
                usize::from(info.digits),
                "{code} rendered"
            );
            checked += 1;
        }
        assert!(checked > 10, "walked only {checked} codes");

        for code in ["USD", "EUR", "THB"] {
            assert_eq!(minor_unit_digits(code).unwrap(), 2, "{code}");
            assert_eq!(rendered_fraction_digits(code), 2, "{code}");
        }
    }

    #[test]
    fn currency_minor_is_exact_where_cldr_49_disagrees_with_old_lists() {
        let en = locale("en-US");

        assert_eq!(
            currency_minor(&en, 123_456, "IQD").unwrap(),
            "IQD\u{a0}123,456"
        );
        assert_eq!(currency_minor(&en, 1_000, "COP").unwrap(), "COP\u{a0}1,000");
        assert_eq!(format("en-US", 1000.5, "COP"), "COP\u{a0}1,001");
    }

    #[test]
    fn the_cache_is_bounded_and_evicts_least_recently_used() {
        let cache: BoundedCache<u32, u32> = BoundedCache::new(3);
        let build = |value: u32| move || Ok::<_, ()>(value);

        for key in 0..3 {
            cache.get_or_build(&key, build(key)).unwrap();
        }
        // Touch 0 so 1 is the oldest, then overflow.
        cache.get_or_build(&0, build(99)).unwrap();
        cache.get_or_build(&3, build(3)).unwrap();

        assert_eq!(cache.len(), 3);
        assert_eq!(*cache.get_or_build(&0, build(99)).unwrap(), 0, "0 survived");
        assert_eq!(*cache.get_or_build(&1, build(11)).unwrap(), 11, "1 evicted");

        for key in 10..100 {
            cache.get_or_build(&key, build(key)).unwrap();
        }
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn currency_minor_is_exact_at_the_i64_extremes() {
        let en = locale("en-US");

        assert_eq!(currency_minor(&en, 1, "USD").unwrap(), "$0.01");
        assert_eq!(currency_minor(&en, 0, "USD").unwrap(), "$0.00");
        assert!(
            currency_minor(&en, i64::MAX, "USD")
                .unwrap()
                .contains("92,233,720,368,547,758.07")
        );
        assert!(
            currency_minor(&en, i64::MIN, "USD")
                .unwrap()
                .contains("92,233,720,368,547,758.08")
        );
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
