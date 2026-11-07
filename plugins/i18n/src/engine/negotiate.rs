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
//! workspace-level pin decision rather than a local one, so neither is
//! touched. Script awareness comes from a small frust-owned table instead
//! ([`likely_script`]): before negotiating, every request without a script
//! gets its likely one filled in (`zh-TW` becomes `zh-Hant-TW`), and after
//! negotiating, any available bundle whose script contradicts the request's
//! is dropped. Without this, `fluent-langneg`'s built-in table does not
//! maximize `zh-TW`/`zh-HK`/`zh-MO`, and the language-only match hands a
//! Traditional-Chinese request a `zh-Hans` bundle. The declared fallback is
//! exempt, so a script mismatch ends at the app's fallback. A region-tagged
//! bundle takes part in the filtering too: it counts as the script its own
//! likely-script row gives it (`zh-CN` is `Hans`, `zh-TW` is `Hant`), so a
//! `zh-TW` request never lands on a `zh-CN` bundle.
//!
//! A script-less available bundle (e.g. `zh`) stays a language-wide match for
//! every request of that language; name bundles with a script (`zh-Hans`,
//! `zh-Hant`) to get script-aware behaviour.

use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use unic_langid::LanguageIdentifier;
use unic_langid::subtags::{Language, Region, Script};

use crate::Locale;

/// The likely script of `language` in `region`, for the languages where the
/// script decides whether text is readable. `None` means "no opinion" and
/// leaves negotiation on language and region alone.
///
/// A deliberate subset, not CLDR's full likely-subtags data:
///
/// - `zh`: `TW`/`HK`/`MO` give `Hant`, as do `US`/`GB` (the regions
///   `fluent-langneg`'s built-in table also maximizes to `Hant`); `CN`/`SG` or
///   no region give `Hans`. Other regions (`zh-AU`) have no opinion.
/// - `sr`: no region, or `RS`/`BA`, gives `Cyrl`. `ME` is left unanswered:
///   its likely script is not settled here, and a guess would reject a bundle
///   of the other script that matched before.
///
/// To extend it, add a row here for another language whose shipped bundles
/// are split by script, plus a test row in this module.
pub(crate) fn likely_script(language: Language, region: Option<Region>) -> Option<Script> {
    let region = region.as_ref().map(Region::as_str);
    let script = match (language.as_str(), region) {
        ("zh", Some("TW" | "HK" | "MO" | "US" | "GB")) => "Hant",
        ("zh", None | Some("CN" | "SG")) => "Hans",
        ("sr", None | Some("RS" | "BA")) => "Cyrl",
        _ => return None,
    };
    script.parse().ok()
}

/// Resolves `requested` against `available`, returning the fallback chain.
///
/// The chain is never empty: `fallback` closes it even when no requested
/// locale matched anything.
pub fn negotiate(
    requested: &[Locale],
    available: &[LanguageIdentifier],
    fallback: &LanguageIdentifier,
) -> Vec<Locale> {
    let requested: Vec<LanguageIdentifier> = requested
        .iter()
        .map(super::lang_id)
        .map(|mut id| {
            if id.script.is_none() {
                id.script = likely_script(id.language, id.region);
            }
            id
        })
        .collect();

    negotiate_languages(
        &requested,
        available,
        Some(fallback),
        NegotiationStrategy::Filtering,
    )
    .into_iter()
    .filter(|candidate| *candidate == fallback || script_compatible(candidate, &requested))
    .cloned()
    .map(Locale::from)
    .collect()
}

/// Whether `candidate` may serve at least one request of its language.
///
/// The candidate's script is its explicit one, else its likely script when it
/// names a region (`zh-CN` is Simplified, `zh-TW` Traditional). A bare
/// language (`zh`) has no script of its own and stays language-wide, as does a
/// candidate whose likely script is unknown; a candidate sharing no language
/// with the requests is never rejected here.
fn script_compatible(candidate: &LanguageIdentifier, requested: &[LanguageIdentifier]) -> bool {
    let script = candidate.script.or_else(|| {
        candidate
            .region
            .and_then(|region| likely_script(candidate.language, Some(region)))
    });
    let Some(script) = script else {
        return true;
    };
    let mut same_language = requested
        .iter()
        .filter(|request| request.language == candidate.language)
        .peekable();
    same_language.peek().is_none()
        || same_language.any(|request| request.script.is_none_or(|wanted| wanted == script))
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

    const BUNDLES: [&str; 5] = ["en", "ja", "zh-Hans", "th", "vi"];

    #[test]
    fn traditional_chinese_requests_never_reach_a_simplified_bundle() {
        for request in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh-Hant-TW"] {
            assert_eq!(chain(&[request], &BUNDLES, "en"), ["en"], "{request}");
        }
    }

    #[test]
    fn simplified_chinese_requests_reach_the_simplified_bundle() {
        for request in ["zh-CN", "zh", "zh-SG", "zh-Hans-CN"] {
            assert_eq!(
                chain(&[request], &BUNDLES, "en"),
                ["zh-Hans", "en"],
                "{request}"
            );
        }
    }

    #[test]
    fn a_script_split_bundle_pair_is_chosen_by_the_request_script() {
        let available = ["zh-Hant", "zh-Hans", "en"];
        assert_eq!(chain(&["zh-TW"], &available, "en"), ["zh-Hant", "en"]);
        assert_eq!(chain(&["zh-CN"], &available, "en"), ["zh-Hans", "en"]);
    }

    #[test]
    fn a_script_mismatched_request_is_skipped_not_the_whole_list() {
        assert_eq!(
            chain(&["zh-TW", "ja-JP"], &["en", "ja", "zh-Hans"], "en"),
            ["ja", "en"]
        );
    }

    #[test]
    fn a_script_less_bundle_matches_every_request_of_its_language() {
        assert_eq!(chain(&["zh-TW"], &["zh", "en"], "en"), ["zh", "en"]);
    }

    #[test]
    fn a_fallback_with_a_different_script_still_closes_the_chain() {
        assert_eq!(
            chain(&["zh-TW"], &["zh-Hans", "en"], "zh-Hans"),
            ["zh-Hans"]
        );
    }

    #[test]
    fn likely_script_table_rows() {
        let script = |tag: &str| {
            let locale: Locale = tag.parse().expect("valid locale");
            likely_script(locale.language(), locale.region()).map(|s| s.to_string())
        };
        assert_eq!(script("zh-TW").as_deref(), Some("Hant"));
        assert_eq!(script("zh-HK").as_deref(), Some("Hant"));
        assert_eq!(script("zh-MO").as_deref(), Some("Hant"));
        assert_eq!(script("zh-CN").as_deref(), Some("Hans"));
        assert_eq!(script("zh-SG").as_deref(), Some("Hans"));
        assert_eq!(script("zh").as_deref(), Some("Hans"));
        assert_eq!(script("zh-US").as_deref(), Some("Hant"));
        assert_eq!(script("zh-GB").as_deref(), Some("Hant"));
        assert_eq!(script("zh-AU").as_deref(), None);
        assert_eq!(script("sr").as_deref(), Some("Cyrl"));
        assert_eq!(script("sr-RS").as_deref(), Some("Cyrl"));
        assert_eq!(script("sr-ME").as_deref(), None);
        assert_eq!(script("sr-BA").as_deref(), Some("Cyrl"));
        assert_eq!(script("de-AT"), None);
    }

    #[test]
    fn region_tagged_bundles_are_matched_by_their_likely_script() {
        assert_eq!(chain(&["zh-TW"], &["zh-CN", "en"], "en"), ["en"]);
        assert_eq!(chain(&["zh-CN"], &["zh-TW", "en"], "en"), ["en"]);
        assert_eq!(
            chain(&["zh-TW"], &["zh-TW", "zh-CN", "en"], "en"),
            ["zh-TW", "en"]
        );
        assert_eq!(chain(&["zh-HK"], &["zh-TW", "en"], "en"), ["zh-TW", "en"]);
    }

    #[test]
    fn a_bare_language_request_reaches_a_region_tagged_bundle() {
        assert_eq!(chain(&["zh"], &["zh-CN", "en"], "en"), ["zh-CN", "en"]);
    }

    #[test]
    fn regions_without_a_likely_script_keep_matching_by_language() {
        assert_eq!(chain(&["de-AT"], &["de-DE", "en"], "en"), ["de-DE", "en"]);
    }

    #[test]
    fn serbian_montenegro_matches_either_script_bundle() {
        assert_eq!(
            chain(&["sr-ME"], &["sr-Latn", "en"], "en"),
            ["sr-Latn", "en"]
        );
        assert_eq!(
            chain(&["sr-ME"], &["sr-Cyrl", "en"], "en"),
            ["sr-Cyrl", "en"]
        );
    }

    #[test]
    fn zh_us_and_zh_gb_negotiate_as_traditional() {
        for request in ["zh-US", "zh-GB"] {
            assert_eq!(
                chain(&[request], &["zh-Hans", "en"], "en"),
                ["en"],
                "{request}"
            );
            assert_eq!(
                chain(&[request], &["zh-Hant", "en"], "en"),
                ["zh-Hant", "en"],
                "{request}"
            );
        }
    }
}
