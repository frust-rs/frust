//! [`Locale`]: a thin, `Display`/`FromStr` newtype over
//! `unic_langid::LanguageIdentifier` — the BCP-47 locale identifier every
//! `frust-i18n` API (bundle lookup, negotiation, detection) speaks.

use std::fmt;
use std::str::FromStr;

use unic_langid::LanguageIdentifier;
use unic_langid::subtags::{Language, Region, Script};

/// A BCP-47 locale identifier (e.g. `en-US`, `de-CH`, `zh-Hans-CN`).
///
/// A thin wrapper over [`unic_langid::LanguageIdentifier`] rather than a
/// re-export — `frust-i18n`'s public API names `Locale`, keeping
/// `unic_langid` an implementation detail callers never need to depend on
/// directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Locale(LanguageIdentifier);

impl Locale {
    /// The primary language subtag (e.g. `en` in `en-US`).
    pub fn language(&self) -> Language {
        self.0.language
    }

    /// The script subtag, if present (e.g. `Hans` in `zh-Hans-CN`).
    pub fn script(&self) -> Option<Script> {
        self.0.script
    }

    /// The region subtag, if present (e.g. `US` in `en-US`).
    pub fn region(&self) -> Option<Region> {
        self.0.region
    }

    /// The underlying `unic_langid` identifier — the lossless route back for
    /// negotiation and ICU bridging (carries locale variants the subtag
    /// accessors above do not expose).
    pub(crate) fn as_lang_id(&self) -> &LanguageIdentifier {
        &self.0
    }
}

impl FromStr for Locale {
    type Err = unic_langid::LanguageIdentifierError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        LanguageIdentifier::from_str(s).map(Locale)
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<LanguageIdentifier> for Locale {
    fn from(id: LanguageIdentifier) -> Self {
        Locale(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_language_and_region() {
        let locale: Locale = "de-CH".parse().expect("valid locale");
        assert_eq!(locale.language().as_str(), "de");
        assert_eq!(locale.region().expect("region present").as_str(), "CH");
        assert_eq!(locale.to_string(), "de-CH");
    }
}
