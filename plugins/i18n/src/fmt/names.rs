//! Language names: "English", "ไทย", "Tiếng Việt" — for a language picker.
//!
//! A language's name in its own language is its endonym, and that is what a
//! picker shows: a Thai user looking for their language scans for "ไทย", not
//! "Thai". The string is CLDR's, so casing follows each language's own
//! convention (`es` is "español", lower case).
//!
//! Names are looked up by **language** only; the script and region of either
//! locale are ignored, so `en-GB` and `en-US` name `fr` identically.

use std::sync::LazyLock;

use icu_experimental::displaynames::multi::LanguageDisplayNames;
use icu_experimental::displaynames::{DisplayNamesOptions, DisplayNamesPreferences};

use crate::{I18nError, Locale};

use super::bridge::{self, Cache};

type NamesCache = Cache<Locale, LanguageDisplayNames>;

static NAMES: LazyLock<NamesCache> = LazyLock::new(Cache::new);

/// The name of `of`'s language, written in `in_locale`'s language.
///
/// # Errors
///
/// [`I18nError::Format`] when `in_locale`'s display-name data fails to load,
/// or when it has no name for `of`'s language.
pub fn language_name(of: &Locale, in_locale: &Locale) -> Result<String, I18nError> {
    let names = NAMES
        .get_or_build(in_locale, || {
            LanguageDisplayNames::try_new(
                DisplayNamesPreferences::from(&bridge::lang_id(in_locale)),
                DisplayNamesOptions::default(),
            )
        })
        .map_err(|error| {
            I18nError::Format(format!(
                "language-name data for `{in_locale}` failed to load: {error}"
            ))
        })?;

    names
        .of(bridge::lang_id(of).language)
        .map(str::to_owned)
        .ok_or_else(|| {
            I18nError::Format(format!(
                "`{in_locale}` has no name for the language of `{of}`"
            ))
        })
}

/// The name of `of`'s language in that language itself.
///
/// # Errors
///
/// As [`language_name`].
pub fn language_endonym(of: &Locale) -> Result<String, I18nError> {
    language_name(of, of)
}
