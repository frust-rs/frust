//! Locale-aware list joining ("Pool, Shower, and Wi-Fi").
//!
//! One ICU4X `ListFormatter` is built per (locale, style, length) and kept
//! for the process's life, like every other formatter in this module.
//!
//! The joined string is ICU's, byte for byte. CLDR's conventions differ
//! enough that none of them is worth second-guessing: English takes a serial
//! comma, Thai has none and glues the conjunction to the last item, Japanese
//! uses an ideographic comma and no conjunction.

use std::sync::LazyLock;

use icu_list::options::{ListFormatterOptions, ListLength as IcuLength};
use icu_list::{ListFormatter, ListFormatterPreferences};

use crate::{I18nError, Locale};

use super::bridge::{self, Cache};

/// Which conjunction a list uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListStyle {
    /// "A, B, and C".
    And,
    /// "A, B, or C".
    Or,
    /// A bare unit list with no conjunction ("5 ft, 2 in").
    Unit,
}

/// How much room the list may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListLength {
    /// The spelled-out form.
    Wide,
    /// A shortened form.
    Short,
    /// The most compact form.
    Narrow,
}

type ListCache = Cache<(Locale, ListStyle, ListLength), ListFormatter>;

static LISTS: LazyLock<ListCache> = LazyLock::new(Cache::new);

/// Joins `items` in `locale`'s list pattern.
///
/// An empty list is the empty string and a single item is returned as-is.
///
/// # Errors
///
/// [`I18nError::Format`] when the locale's list data fails to load.
pub fn list(
    locale: &Locale,
    items: &[&str],
    style: ListStyle,
    length: ListLength,
) -> Result<String, I18nError> {
    let formatter = LISTS
        .get_or_build(&(locale.clone(), style, length), || {
            let prefs = ListFormatterPreferences::from(&bridge::lang_id(locale));
            let options = ListFormatterOptions::default().with_length(match length {
                ListLength::Wide => IcuLength::Wide,
                ListLength::Short => IcuLength::Short,
                ListLength::Narrow => IcuLength::Narrow,
            });
            match style {
                ListStyle::And => ListFormatter::try_new_and(prefs, options),
                ListStyle::Or => ListFormatter::try_new_or(prefs, options),
                ListStyle::Unit => ListFormatter::try_new_unit(prefs, options),
            }
        })
        .map_err(|error| {
            I18nError::Format(format!("list data for `{locale}` failed to load: {error}"))
        })?;

    Ok(formatter.format(items.iter()).to_string())
}
