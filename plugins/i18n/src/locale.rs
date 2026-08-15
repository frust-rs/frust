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

    /// Composes `language` with `region`, with no script/variant subtags —
    /// the crate-private seam behind `reactive::I18n::format_locale`'s
    /// composition rule (graft a requested tag's REGION onto the message
    /// locale's language, e.g. `en` + `TH` region → `en-TH`) and
    /// `reactive::I18n::set_locale`'s region-retention rule (graft a
    /// previously detected region onto a new region-less request, e.g.
    /// `de-CH` retained + `set_locale("de")` → `de-CH` again). Both call
    /// sites compose a *bare* language tag (the message locale, or a fresh
    /// `set_locale` request with no region of its own), so dropping
    /// script/variants here costs nothing in practice — never exposed
    /// publicly, since a caller only ever reaches a composed locale through
    /// those two methods, never by constructing one directly.
    ///
    /// `frust-api`-gated: `reactive` (its only caller) doesn't compile
    /// without that feature — same gate, so `--no-default-features` never
    /// sees this as unused dead code.
    #[cfg(feature = "frust-api")]
    pub(crate) fn compose_region(language: Language, region: Region) -> Self {
        Locale(LanguageIdentifier::from_parts(
            language,
            None,
            Some(region),
            &[],
        ))
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
