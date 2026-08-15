//! Requested-vs-available locale negotiation.
//!
//! Wraps `fluent_langneg::negotiate_languages` under
//! [`NegotiationStrategy::Filtering`] — the strategy that returns *every*
//! available locale matching a request, best first, rather than a single
//! winner: that ranked list is exactly the fallback chain
//! [`resolve`](super::resolve) walks. The declared fallback is passed as the
//! negotiator's `default`, so it is appended once when it isn't already in
//! the result, and is the whole chain when nothing matches at all.
//!
//! `fluent-langneg`'s `cldr` feature (full CLDR likely-subtags) is off, so
//! step 3/5 of its algorithm uses the crate's built-in approximation table.
//! Turning it on would mean enabling `unic-langid/likelysubtags`, a
//! workspace-level pin decision rather than a local one.

use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use unic_langid::LanguageIdentifier;

use crate::Locale;

/// Resolves `requested` against `available`, returning the fallback chain.
///
/// The chain is never empty: `fallback` closes it even when no requested
/// locale matched anything.
pub fn negotiate(
    requested: &[Locale],
    available: &[LanguageIdentifier],
    fallback: &LanguageIdentifier,
) -> Vec<Locale> {
    let requested: Vec<LanguageIdentifier> = requested.iter().map(super::lang_id).collect();

    negotiate_languages(
        &requested,
        available,
        Some(fallback),
        NegotiationStrategy::Filtering,
    )
    .into_iter()
    .cloned()
    .map(Locale::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(tags: &[&str]) -> Vec<LanguageIdentifier> {
        tags.iter()
            .map(|tag| tag.parse().expect("valid language identifier"))
            .collect()
    }

    fn locales(tags: &[&str]) -> Vec<Locale> {
        tags.iter()
            .map(|tag| tag.parse().expect("valid locale"))
            .collect()
    }

    fn chain(requested: &[&str], available: &[&str], fallback: &str) -> Vec<String> {
        let fallback: LanguageIdentifier = fallback.parse().expect("valid fallback");
        negotiate(&locales(requested), &ids(available), &fallback)
            .iter()
            .map(|locale| locale.to_string())
            .collect()
    }

    #[test]
    fn regional_request_falls_back_to_its_language_then_the_default() {
        assert_eq!(chain(&["de-CH"], &["de", "en"], "en"), ["de", "en"]);
    }

    #[test]
    fn regional_request_prefers_the_generic_language_over_a_sibling_region() {
        // `pt` matches `pt-BR` as a range (region wildcard) before `pt-PT`,
        // which is only reached by the region-as-range step; `en` closes the
        // chain as the declared fallback.
        assert_eq!(
            chain(&["pt-BR"], &["pt-PT", "pt", "en"], "en"),
            ["pt", "pt-PT", "en"]
        );
    }

    #[test]
    fn exact_match_leads_the_chain() {
        assert_eq!(chain(&["de"], &["en", "de"], "en"), ["de", "en"]);
    }

    #[test]
    fn empty_intersection_is_the_fallback_alone() {
        assert_eq!(chain(&["ja"], &["de", "en"], "en"), ["en"]);
    }

    #[test]
    fn no_request_is_the_fallback_alone() {
        assert_eq!(chain(&[], &["de", "en"], "en"), ["en"]);
    }

    #[test]
    fn multiple_requests_keep_their_preference_order() {
        assert_eq!(
            chain(&["fr", "de"], &["de", "en", "fr"], "en"),
            ["fr", "de", "en"]
        );
    }

    #[test]
    fn a_fallback_already_matched_is_not_repeated() {
        assert_eq!(chain(&["en-GB"], &["en", "de"], "en"), ["en"]);
    }
}
