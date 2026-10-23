//! Choosing the table for a locale and providing it through the theme.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use frust::{Theme, provide_context, use_context};
use frust_i18n::{I18n, Locale};
use frust_material::{DatePickerStrings, DatePickerStringsOwned, MaterialStrings};

use crate::tables::{de, en, es, fr, ja, th, vi};
use names::calendar_names;

/// Month and weekday names read from CLDR through ICU4X.
mod names {
    use icu_datetime::fieldsets::{E, M};
    use icu_datetime::input::Date;
    use icu_datetime::{DateTimeFormatter, DateTimeFormatterPreferences};
    use icu_locale_core::LanguageIdentifier;

    /// Full months, abbreviated months and abbreviated weekdays (Sunday first).
    pub(super) struct CalendarNames {
        pub months: [String; 12],
        pub month_abbreviations: [String; 12],
        pub weekday_abbreviations: [String; 7],
    }

    /// 2026-01-04 is a Sunday.
    const SUNDAY: (i32, u8, u8) = (2026, 1, 4);

    /// CLDR names for `language` (a bare language subtag), or `None` when ICU4X
    /// cannot load them.
    pub(super) fn calendar_names(language: &str) -> Option<CalendarNames> {
        let id: LanguageIdentifier = language.parse().ok()?;
        let prefs = DateTimeFormatterPreferences::from(&id);
        let long = DateTimeFormatter::try_new(prefs, M::long()).ok()?;
        let abbreviated = DateTimeFormatter::try_new(prefs, M::medium()).ok()?;
        let weekday = DateTimeFormatter::try_new(prefs, E::medium()).ok()?;

        let month = |formatter: &DateTimeFormatter<M>, month: u8| {
            Date::try_new_iso(2026, month, 1)
                .ok()
                .map(|d| formatter.format(&d).to_string())
        };
        let mut months = Vec::with_capacity(12);
        let mut month_abbreviations = Vec::with_capacity(12);
        for m in 1..=12u8 {
            months.push(month(&long, m)?);
            month_abbreviations.push(month(&abbreviated, m)?);
        }
        let mut weekdays = Vec::with_capacity(7);
        for offset in 0..7u8 {
            let day = Date::try_new_iso(SUNDAY.0, SUNDAY.1, SUNDAY.2 + offset).ok()?;
            weekdays.push(weekday.format(&day).to_string());
        }

        Some(CalendarNames {
            months: months.try_into().ok()?,
            month_abbreviations: month_abbreviations.try_into().ok()?,
            weekday_abbreviations: weekdays.try_into().ok()?,
        })
    }
}

/// Finished tables by language. Building one reads CLDR and leaks the owned
/// month and weekday strings once (see [`DatePickerStringsOwned`]), so each
/// language is built a single time per process.
static TABLES: Mutex<Option<HashMap<String, MaterialStrings>>> = Mutex::new(None);

/// The hand-written table for `language`, or `None` when unsupported.
fn hand_written(language: &str) -> Option<MaterialStrings> {
    Some(match language {
        "en" => en::strings(),
        "th" => th::strings(),
        "vi" => vi::strings(),
        "ja" => ja::strings(),
        "es" => es::strings(),
        "de" => de::strings(),
        "fr" => fr::strings(),
        _ => return None,
    })
}

fn build_table(language: &str) -> MaterialStrings {
    let Some(mut strings) = hand_written(language) else {
        return MaterialStrings::ENGLISH;
    };
    if let Some(names) = calendar_names(language) {
        let mut owned = DatePickerStringsOwned::from(strings.date_picker);
        owned.months = names.months.map(Cow::Owned);
        owned.month_abbreviations = names.month_abbreviations.map(Cow::Owned);
        owned.weekday_abbreviations = names.weekday_abbreviations.map(Cow::Owned);
        strings.date_picker = DatePickerStrings::from(owned);
    }
    strings
}

/// The [`MaterialStrings`] for `locale`'s language, falling back to
/// [`MaterialStrings::ENGLISH`] for a language without a table.
///
/// Region and script are ignored: `de-CH` and `de` share one table. Month and
/// weekday names come from CLDR; every other string is hand-written.
pub fn material_strings_for(locale: &Locale) -> MaterialStrings {
    let language = locale.language();
    let language = language.as_str();
    let mut guard = TABLES.lock().unwrap_or_else(PoisonError::into_inner);
    guard
        .get_or_insert_with(HashMap::new)
        .entry(language.to_owned())
        .or_insert_with(|| build_table(language))
        .clone()
}

/// `theme` with the [`MaterialStrings`] extension replaced by the table for
/// `i18n`'s active locale. A tracked read of the locale signal.
///
/// [`provide_localized_strings`] is all a component tree needs; this is for
/// an app that manages the shell theme itself (`set_app_theme`) and wants the
/// theme it hands over to carry the same table.
pub fn localized_theme(mut theme: Theme, i18n: &I18n) -> Theme {
    let strings = material_strings_for(&i18n.locale());
    theme.extensions.insert(strings);
    theme
}

/// Provides the ambient [`Theme`] with the Material strings for the active
/// locale.
///
/// Call it from a component's `build`: it reads the locale signal, so every
/// [`I18n::set_locale`] rebuilds that component and provides the new table.
/// The base theme is the ambient one, or the Material baseline when none is
/// provided yet. Every Material widget created below resolves its strings
/// from the provided theme when its view is created, so this call alone
/// localizes the subtree; no shell theme push is needed.
pub fn provide_localized_strings(i18n: &I18n) {
    let base = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    provide_context(localized_theme(base, i18n));
}
