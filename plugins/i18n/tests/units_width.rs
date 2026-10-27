//! Long/short/narrow unit widths, checked against ICU4X directly.
//!
//! `frust_i18n::fmt::units` builds the data attribute `<width>-<unit>`
//! (`long-kilometer`) itself. This test loads the same attribute straight
//! from `icu_experimental`'s baked data, bypassing that loader, and
//! formats with ICU4X's own typed `CategorizedFormatter` for the one unit
//! it can construct (`Length::meter()`), so a disagreement between the two
//! paths shows up as a failure rather than as a silent pin.

#![cfg(feature = "formatting")]

use frust_i18n::Locale;
use frust_i18n::fmt::{self, Unit, UnitStyle};
use icu_decimal::input::Decimal;
use icu_decimal::{DecimalFormatter, DecimalFormatterPreferences};
use icu_experimental::dimension::provider::units::categorized_display_names::UnitsNamesLengthCoreV1;
use icu_experimental::dimension::provider::units::categorized_display_names::UnitsNamesLengthExtendedV1;
use icu_experimental::dimension::provider::units::categorized_display_names::UnitsNamesLengthOutlierV1;
use icu_experimental::dimension::units::categorized_formatter::CategorizedFormatter;
use icu_experimental::dimension::units::options::{UnitsFormatterOptions, Width};
use icu_experimental::measure::category::Length;
use icu_locale_core::locale;
use icu_plurals::{PluralRules, PluralRulesPreferences};
use icu_provider::marker::DataMarkerExt;
use icu_provider::prelude::*;

const STYLES: [(UnitStyle, &str, Width); 3] = [
    (UnitStyle::Long, "long", Width::Long),
    (UnitStyle::Short, "short", Width::Short),
    (UnitStyle::Narrow, "narrow", Width::Narrow),
];

/// Loads `<width>-<unit_id>` from the baked length markers (core, then
/// extended, then outlier) and renders `value` with it.
fn icu_by_attribute(tag: &icu_locale_core::Locale, attribute: &str, value: &Decimal) -> String {
    let lang = tag.id.clone();
    let attribute = DataMarkerAttributes::try_from_str(attribute).expect("attribute");
    let decimal =
        DecimalFormatter::try_new(DecimalFormatterPreferences::from(&lang), Default::default())
            .expect("decimal");
    let plurals =
        PluralRules::try_new_cardinal(PluralRulesPreferences::from(&lang)).expect("plurals");

    macro_rules! load {
        ($marker:ty) => {{
            let data_locale = <$marker>::make_locale((&lang).into());
            DataProvider::<$marker>::load(
                &icu_experimental::provider::Baked,
                DataRequest {
                    id: DataIdentifierBorrowed::for_marker_attributes_and_locale(
                        attribute,
                        &data_locale,
                    ),
                    ..Default::default()
                },
            )
            .map(|response| {
                response
                    .payload
                    .get()
                    .get(value.into(), &plurals)
                    .interpolate((decimal.format(value),))
                    .to_string()
            })
        }};
    }
    load!(UnitsNamesLengthCoreV1)
        .or_else(|_| load!(UnitsNamesLengthExtendedV1))
        .or_else(|_| load!(UnitsNamesLengthOutlierV1))
        .expect("length data for the attribute")
}

fn frust_locale(tag: &str) -> Locale {
    tag.parse().expect("valid locale")
}

#[test]
fn two_kilometers_agree_with_icu_at_every_width() {
    let en = frust_locale("en");
    let icu_locale = locale!("en");
    let two = Decimal::from(2u8);

    for (style, width, _) in STYLES {
        let frust = fmt::units(&en, 2.0, Unit::Kilometer, style).expect("frust formats");
        let icu = icu_by_attribute(&icu_locale, &format!("{width}-kilometer"), &two);
        println!("en {width:>6} 2 km: frust={frust:?} icu={icu:?}");
        assert_eq!(frust, icu, "{width}-kilometer");
    }
}

#[test]
fn meters_agree_with_icus_typed_formatter_at_every_width() {
    let en = frust_locale("en");
    let two = Decimal::from(2u8);

    for (style, width, icu_width) in STYLES {
        let frust = fmt::units(&en, 2.0, Unit::Meter, style).expect("frust formats");
        let typed = CategorizedFormatter::<Length>::try_new_core(
            locale!("en").into(),
            Length::meter(),
            UnitsFormatterOptions::from(icu_width),
        )
        .expect("typed formatter");
        let icu = typed.format_fixed_decimal(&two).to_string();
        println!("en {width:>6} 2 m: frust={frust:?} typed-icu={icu:?}");
        assert_eq!(frust, icu, "{width}-meter");
    }
}
