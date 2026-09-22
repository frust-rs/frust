//! System-locale detection.
//!
//! Routes by `#[cfg(target_os = ...)]` to a real backend — Android
//! ([`android`], a JNI read of `Resources.getSystem().getConfiguration()
//! .getLocales()`), Apple ([`apple`], `objc2-foundation`'s
//! `NSLocale.preferredLanguages`, iOS only — macOS shares the desktop
//! backend instead, see `Cargo.toml`'s target-dependency comments), and
//! desktop ([`desktop`], `sys-locale`) — the same target-gated-FFI shape
//! every plugin's `Cargo.toml` splits on (`docs/PLUGINS_ARCHITECTURE.md`'s
//! Layer Dependencies). The three cfg arms below (`target_os = "android"` /
//! `target_os = "ios"` / their complement) are jointly exhaustive over every
//! `target_os` this crate compiles for today, so
//! [`crate::I18nError::UnsupportedPlatform`] is never raised from here — it
//! stays reserved for a future backend narrowing (e.g. carving a real gap
//! out of the desktop catch-all).
//!
//! [`system_locales`] re-queries the OS on **every** call — it never caches
//! anything at this layer. That is deliberate: an Android configuration
//! change (the user switches system language while the app is
//! backgrounded/foregrounded) must be visible on the very next call with no
//! extra event plumbing required.

use crate::{I18nError, Locale};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;

/// Returns the platform's configured locale list, most-preferred first.
///
/// Always re-queries the OS (see the module doc) — never cached.
///
/// # Errors
///
/// [`I18nError::Detection`] if the platform backend reports nothing
/// parseable at all — a genuinely empty result, not merely a tag or two
/// that individually failed to parse (those are skipped silently; see
/// [`parse_tags`]).
pub fn system_locales() -> Result<Vec<Locale>, I18nError> {
    #[cfg(target_os = "android")]
    let raw = android::raw_locale_tags()?;
    #[cfg(target_os = "ios")]
    let raw = apple::raw_locale_tags();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let raw = desktop::raw_locale_tags();

    parse_tags(raw)
}

/// Parses a platform's raw BCP-47 tag list into [`Locale`]s.
///
/// - An unparseable tag (`Locale::from_str`'s
///   `unic_langid::LanguageIdentifierError`) is skipped and logged, not
///   fatal — one malformed entry from a platform quirk shouldn't sink the
///   whole preference list.
/// - Duplicates are removed while preserving first-seen order — Android's
///   `LocaleList`, `NSLocale.preferredLanguages`, and `sys-locale` can each
///   legitimately report the same locale more than once (a canonicalized
///   `Locale` comparison, so `en-us`/`en-US` also collapse to one entry).
/// - An empty **result** (nothing survived parsing, including the trivial
///   case of no input at all) is [`I18nError::Detection`] — the caller has
///   nothing to negotiate against.
fn parse_tags(tags: impl IntoIterator<Item = String>) -> Result<Vec<Locale>, I18nError> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for tag in tags {
        match tag.parse::<Locale>() {
            Ok(locale) => {
                if seen.insert(locale.clone()) {
                    out.push(locale);
                }
            }
            Err(err) => {
                log::debug!("skipping unparseable system locale tag '{tag}': {err}");
            }
        }
    }
    if out.is_empty() {
        return Err(I18nError::Detection(
            "the platform reported no parseable locale tags".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_unparseable_tags_and_dedupes_preserving_order() {
        let tags = ["en-US", "zz-!!", "en-US", "de"].map(String::from);
        let locales = parse_tags(tags).expect("at least one tag parses");
        let rendered: Vec<String> = locales.iter().map(ToString::to_string).collect();
        assert_eq!(rendered, vec!["en-US".to_string(), "de".to_string()]);
    }

    #[test]
    fn no_input_is_a_detection_error() {
        assert!(matches!(
            parse_tags(std::iter::empty::<String>()),
            Err(I18nError::Detection(_))
        ));
    }

    #[test]
    fn all_unparseable_input_is_a_detection_error() {
        assert!(matches!(
            parse_tags(["!!!", "###", ""].map(String::from)),
            Err(I18nError::Detection(_))
        ));
    }

    #[test]
    fn case_variants_of_the_same_locale_dedupe() {
        let tags = ["en-us", "en-US"].map(String::from);
        let locales = parse_tags(tags).expect("at least one tag parses");
        assert_eq!(locales.len(), 1);
    }

    /// Desktop smoke test: on a real host (this workspace's `cargo test`
    /// runs on desktop, never Android/iOS) `system_locales` must either
    /// report a real, parseable locale list, or fail with the documented
    /// [`I18nError::Detection`] — never anything else, and never a panic.
    /// A bare `LANG=C.UTF-8` host with every other locale variable unset
    /// is a legitimate instance of the latter (see `detect::desktop`'s
    /// module doc: `C`/`POSIX` carries no usable language, so
    /// `raw_locale_tags` correctly reports nothing to negotiate against),
    /// so non-emptiness of an `Ok` result is asserted rather than the call
    /// being required to succeed outright — but that assertion is not
    /// vacuous: `parse_tags` (exercised directly by this module's other
    /// tests) already guarantees an `Ok` is never empty, so this test's
    /// real job is catching a regression that returns a *wrong* error
    /// variant (e.g. `UnsupportedPlatform`) for what should be a
    /// `Detection` failure, or that panics instead of erroring.
    #[test]
    fn system_locales_reports_parseable_locales_or_a_documented_detection_error() {
        match system_locales() {
            Ok(locales) => assert!(
                !locales.is_empty(),
                "system_locales' Ok variant must never be empty"
            ),
            Err(I18nError::Detection(_)) => {}
            Err(other) => panic!("unexpected error from system_locales: {other}"),
        }
    }
}
