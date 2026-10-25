//! Unit formatting ("1.5 hr", "2 hours", "90 min") and the duration helper
//! built on it.
//!
//! # `icu_experimental` surface
//!
//! Like [`currency`](super::currency), this file is the whole
//! `UnitsFormatter` surface: callers speak `&Locale`/`f64`/`Decimal`/`String`.
//!
//! # [`Unit`] → CLDR unit id
//!
//! | [`Unit`]     | id            |
//! |--------------|---------------|
//! | `Hour`       | `hour`        |
//! | `Minute`     | `minute`      |
//! | `Day`        | `day`         |
//! | `Kilometer`  | `kilometer`   |
//! | `Meter`      | `meter`       |
//! | `Mile`       | `mile`        |
//!
//! The ids are the bare names the crate's data lookup accepts; the
//! `duration-`-prefixed spellings CLDR's own unit identifiers use are
//! rejected. [`units_by_id`] takes an arbitrary id and answers
//! [`I18nError::UnsupportedUnit`] for one the bundled data has no entry for.
//!
//! # Rendering is CLDR's
//!
//! Widths map to ICU4X's `Long`/`Short`/`Narrow`; a locale may spell two
//! widths identically (Vietnamese all three, Japanese long and short). The
//! value goes through the same fraction rule as [`decimal`](super::decimal)
//! — three digits, no padding — so `1.5` is `1.5` and `2.0` is `2`.
//!
//! # [`duration_minutes`]
//!
//! * under 60 minutes: the minutes (`45` → `45 min`);
//! * a whole number of hours (`60`, `120`, …): the hours (`2 hours`);
//! * anything else: hours to one decimal (`90` → `1.5 hr`; `100` →
//!   `1.7 hr`).

use std::sync::LazyLock;

use icu_decimal::input::Decimal;
use icu_decimal::{DecimalFormatter, DecimalFormatterPreferences};
use icu_experimental::dimension::provider::units::categorized_display_names::{
    UnitsNamesAreaCoreV1, UnitsNamesAreaExtendedV1, UnitsNamesAreaOutlierV1,
    UnitsNamesDurationCoreV1, UnitsNamesDurationExtendedV1, UnitsNamesDurationOutlierV1,
    UnitsNamesLengthCoreV1, UnitsNamesLengthExtendedV1, UnitsNamesLengthOutlierV1,
    UnitsNamesMassCoreV1, UnitsNamesMassExtendedV1, UnitsNamesMassOutlierV1,
    UnitsNamesVolumeCoreV1, UnitsNamesVolumeExtendedV1, UnitsNamesVolumeOutlierV1,
};
use icu_experimental::dimension::provider::units::display_names::UnitsDisplayNames;
use icu_plurals::{PluralRules, PluralRulesPreferences};
use icu_provider::marker::{DataMarkerExt, ErasedMarker};
use icu_provider::prelude::*;

use crate::{I18nError, Locale};

use super::bridge::{self, Cache};
use super::number;

const MINUTES_PER_HOUR: u32 = 60;

/// A measurable unit [`units`] can render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Hours (`hour`).
    Hour,
    /// Minutes (`minute`).
    Minute,
    /// Days (`day`).
    Day,
    /// Kilometers (`kilometer`).
    Kilometer,
    /// Meters (`meter`).
    Meter,
    /// Miles (`mile`).
    Mile,
}

impl Unit {
    /// The CLDR unit id the formatter data is keyed by.
    pub const fn cldr_id(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Day => "day",
            Self::Kilometer => "kilometer",
            Self::Meter => "meter",
            Self::Mile => "mile",
        }
    }
}

/// How much of the unit's name to spell out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitStyle {
    /// The tersest form (`1.5h`).
    Narrow,
    /// An abbreviation (`1.5 hr`).
    Short,
    /// The full word (`1.5 hours`).
    Long,
}

impl UnitStyle {
    /// The width prefix of the data attribute (`short-hour`).
    const fn width(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Short => "short",
            Self::Long => "long",
        }
    }
}

/// Everything needed to render one unit in one locale and width: the
/// unit's plural-keyed display-name patterns, the number formatter and the
/// cardinal plural rules that pick among them.
///
/// `icu_experimental` 0.7 replaced the string-keyed `UnitsFormatter` with a
/// per-category typed formatter that only constructs a handful of units, so
/// this assembles the same parts from the public display-name data markers.
struct UnitFormatter {
    names: DataPayload<ErasedMarker<UnitsDisplayNames<'static>>>,
    decimal: DecimalFormatter,
    plurals: PluralRules,
}

type UnitCache = Cache<(Locale, String, UnitStyle), UnitFormatter>;

static UNITS: LazyLock<UnitCache> = LazyLock::new(Cache::new);

/// Formats `value` of `unit` in `locale`.
///
/// # Errors
///
/// [`I18nError::Format`] for a non-finite `value` or when the locale's data
/// fails to load; [`I18nError::UnsupportedUnit`] when no data exists for the
/// unit.
pub fn units(
    locale: &Locale,
    value: f64,
    unit: Unit,
    style: UnitStyle,
) -> Result<String, I18nError> {
    let decimal = number::to_decimal(value)
        .ok_or_else(|| I18nError::Format(format!("`{value}` is not a finite quantity")))?;
    units_decimal(locale, decimal, unit, style)
}

/// As [`units`], for a quantity already held as a `Decimal`.
///
/// # Errors
///
/// As [`units`], minus the finiteness check.
pub fn units_decimal(
    locale: &Locale,
    value: Decimal,
    unit: Unit,
    style: UnitStyle,
) -> Result<String, I18nError> {
    units_by_id_decimal(locale, value, unit.cldr_id(), style)
}

/// As [`units`], for a unit named by its CLDR id rather than a [`Unit`].
///
/// # Errors
///
/// As [`units`]; an id the bundled data does not know is
/// [`I18nError::UnsupportedUnit`].
pub fn units_by_id(
    locale: &Locale,
    value: f64,
    unit_id: &str,
    style: UnitStyle,
) -> Result<String, I18nError> {
    let decimal = number::to_decimal(value)
        .ok_or_else(|| I18nError::Format(format!("`{value}` is not a finite quantity")))?;
    units_by_id_decimal(locale, decimal, unit_id, style)
}

fn units_by_id_decimal(
    locale: &Locale,
    value: Decimal,
    unit_id: &str,
    style: UnitStyle,
) -> Result<String, I18nError> {
    let key = (locale.clone(), unit_id.to_owned(), style);
    let formatter = UNITS
        .get_or_build(&key, || build_formatter(locale, unit_id, style))
        .map_err(|error| match error.kind {
            DataErrorKind::IdentifierNotFound | DataErrorKind::Custom => {
                I18nError::UnsupportedUnit(unit_id.to_owned())
            }
            _ => I18nError::Format(format!("{locale}: loading unit data: {error}")),
        })?;

    let value = number::standard_fraction(value);
    let pattern = formatter
        .names
        .get()
        .get((&value).into(), &formatter.plurals);
    Ok(pattern
        .interpolate((formatter.decimal.format(&value),))
        .to_string())
}

/// Loads the display names for `unit_id` from whichever category holds it.
fn build_formatter(
    locale: &Locale,
    unit_id: &str,
    style: UnitStyle,
) -> Result<UnitFormatter, DataError> {
    let lang = bridge::lang_id(locale);
    let decimal =
        DecimalFormatter::try_new(DecimalFormatterPreferences::from(&lang), Default::default())?;
    let plurals = PluralRules::try_new_cardinal(PluralRulesPreferences::from(&lang))?;

    let attribute = format!("{}-{unit_id}", style.width());
    let attribute = DataMarkerAttributes::try_from_str(&attribute)
        .map_err(|_| DataError::custom("not a unit data attribute"))?;
    let data_locale = UnitsNamesDurationCoreV1::make_locale((&lang).into());
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attribute, &data_locale),
        ..Default::default()
    };

    let names = load_names(request)?;
    Ok(UnitFormatter {
        names,
        decimal,
        plurals,
    })
}

/// Tries each category's core, extended and outlier markers in turn; the
/// attribute is unique across categories, so at most one holds it.
fn load_names(
    request: DataRequest<'_>,
) -> Result<DataPayload<ErasedMarker<UnitsDisplayNames<'static>>>, DataError> {
    macro_rules! try_markers {
        ($($marker:ty),* $(,)?) => {
            $(
                match DataProvider::<$marker>::load(&icu_experimental::provider::Baked, request) {
                    Ok(response) => return Ok(response.payload.cast()),
                    Err(error) if error.kind == DataErrorKind::IdentifierNotFound => {}
                    Err(error) => return Err(error),
                }
            )*
        };
    }
    try_markers!(
        UnitsNamesDurationCoreV1,
        UnitsNamesLengthCoreV1,
        UnitsNamesMassCoreV1,
        UnitsNamesAreaCoreV1,
        UnitsNamesVolumeCoreV1,
        UnitsNamesDurationExtendedV1,
        UnitsNamesLengthExtendedV1,
        UnitsNamesMassExtendedV1,
        UnitsNamesAreaExtendedV1,
        UnitsNamesVolumeExtendedV1,
        UnitsNamesDurationOutlierV1,
        UnitsNamesLengthOutlierV1,
        UnitsNamesMassOutlierV1,
        UnitsNamesAreaOutlierV1,
        UnitsNamesVolumeOutlierV1,
    );
    Err(DataErrorKind::IdentifierNotFound.into_error())
}

/// Formats a duration of `minutes` as minutes, whole hours, or hours with
/// one decimal — see the module doc for the rule.
///
/// # Errors
///
/// As [`units`].
pub fn duration_minutes(
    locale: &Locale,
    minutes: u32,
    style: UnitStyle,
) -> Result<String, I18nError> {
    if minutes < MINUTES_PER_HOUR {
        return units_decimal(locale, Decimal::from(minutes), Unit::Minute, style);
    }
    if minutes.is_multiple_of(MINUTES_PER_HOUR) {
        return units_decimal(
            locale,
            Decimal::from(minutes / MINUTES_PER_HOUR),
            Unit::Hour,
            style,
        );
    }

    // One decimal digit, rounded half-expand on the exact integer ratio.
    let tenths =
        (u64::from(minutes) * 10 + u64::from(MINUTES_PER_HOUR) / 2) / u64::from(MINUTES_PER_HOUR);
    let mut hours = Decimal::from(tenths);
    hours.multiply_pow10(-1);
    units_decimal(locale, hours, Unit::Hour, style)
}
