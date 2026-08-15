//! [`Locale`]: a thin, `Display`/`FromStr` newtype over
//! `unic_langid::LanguageIdentifier` — the BCP-47 locale identifier every
//! `frust-i18n` API (bundle lookup, negotiation, detection) speaks.

use std::fmt;
use std::str::FromStr;

use unic_langid::LanguageIdentifier;
#[cfg(feature = "frust-api")]
use unic_langid::subtags::Variant;
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

    /// Grafts `region` onto `base`, preserving every other subtag `base`
    /// carries (script, variants) — only the region subtag is added or
    /// replaced. The crate-private seam behind
    /// `reactive::I18n::format_locale`'s composition rule (graft a requested
    /// tag's REGION onto the active message locale, e.g. `en` + `TH` region
    /// → `en-TH`, or `zh-Hant` + `US` region → `zh-Hant-US`) and
    /// `reactive::I18n::set_locale`'s region-retention rule (graft a
    /// previously detected region onto a new region-less request, e.g.
    /// `de-CH` retained + `set_locale("de")` → `de-CH` again, or a retained
    /// `US` region + `set_locale("zh-Hant")` → `zh-Hant-US`).
    ///
    /// An earlier revision of this doc claimed dropping script/variants here
    /// "costs nothing in practice", on the premise that both call sites only
    /// ever compose a *bare* language tag. That premise was false: a
    /// `set_locale` caller's own requested tag, and `format_locale`'s active
    /// message locale, can each carry a script (`zh-Hant`, `sr-Latn`, ...)
    /// whenever the app registers script-qualified bundle locales — and
    /// silently dropping it here mangled `set_locale("zh-Hant")` (plus a
    /// retained region) into `zh-<region>` *before* negotiation ever ran,
    /// negotiating the wrong script's bundle. Preserving `base`'s script and
    /// variants is what keeps both call sites correct in that case; never
    /// exposed publicly, since a caller only ever reaches a composed locale
    /// through those two methods, never by constructing one directly.
    ///
    /// `frust-api`-gated: `reactive` (its only caller) doesn't compile
    /// without that feature — same gate, so `--no-default-features` never
    /// sees this as unused dead code.
    #[cfg(feature = "frust-api")]
    pub(crate) fn compose_region(base: &Locale, region: Region) -> Self {
        let id = base.as_lang_id();
        let variants: Vec<Variant> = id.variants().copied().collect();
        Locale(LanguageIdentifier::from_parts(
            id.language,
            id.script,
            Some(region),
            &variants,
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
