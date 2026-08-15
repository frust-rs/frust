//! The desktop detection backend — `sys-locale`, covering every target that
//! is neither Android nor iOS (macOS included: `NSLocale` lives in
//! [`crate::detect::apple`], which is iOS-only, per this crate's `Cargo.toml`
//! design note).
//!
//! `sys_locale::get_locales()` is already BCP-47-normalized on every arm it
//! supports — it converts POSIX-style tags (`en_US.UTF-8`) itself, so this
//! backend doesn't re-implement that. Its own crate doc still allows for a
//! completely empty result on a platform/environment it has no reader for
//! (a minimal container with no locale subsystem, for instance), so this
//! backend falls back, in order: `sys_locale::get_locales()` (the full
//! preference list) → `sys_locale::get_locale()` (its own single-value
//! convenience, kept as an explicit fallback per this crate's design even
//! though it is `get_locales().next()` under the hood) → a last-resort
//! `LANG`-style POSIX env-var read.

/// `sys_locale::get_locales()`, falling back to `sys_locale::get_locale()`
/// then a `LANG`-style env read only when the list comes back empty (see
/// the module doc).
pub(crate) fn raw_locale_tags() -> Vec<String> {
    let locales: Vec<String> = sys_locale::get_locales().collect();
    if !locales.is_empty() {
        return locales;
    }
    if let Some(locale) = sys_locale::get_locale() {
        return vec![locale];
    }
    env_locale_fallback().into_iter().collect()
}

/// A last-resort `LANG`-style POSIX locale env-var read (`en_US.UTF-8`,
/// `de_DE`, `C`/`POSIX`), consulted in the order glibc itself checks
/// (`LC_ALL` overrides `LC_MESSAGES` overrides `LANG`) — only reached when
/// `sys_locale` itself reports nothing.
fn env_locale_fallback() -> Option<String> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(var)
            && let Some(tag) = posix_to_bcp47(&value)
        {
            return Some(tag);
        }
    }
    None
}

/// Converts a POSIX locale string (`en_US.UTF-8`, `de_DE@euro`) to a
/// BCP-47-shaped candidate tag (`en-US`, `de-DE`) — strips the encoding
/// (after `.`) and modifier (after `@`), then swaps `_` for `-`. The
/// language-neutral `C`/`POSIX` values carry no usable language subtag and
/// are skipped (`None`) rather than mistranslated.
fn posix_to_bcp47(value: &str) -> Option<String> {
    let base = value.split('.').next().unwrap_or(value);
    let base = base.split('@').next().unwrap_or(base);
    if base.is_empty() || base.eq_ignore_ascii_case("C") || base.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    Some(base.replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_locales_are_non_empty_and_parseable() {
        let tags = raw_locale_tags();
        assert!(!tags.is_empty(), "no locales were returned");
        for tag in &tags {
            tag.parse::<crate::Locale>()
                .unwrap_or_else(|err| panic!("'{tag}' did not parse as a BCP-47 tag: {err}"));
        }
    }

    #[test]
    fn posix_tags_normalize_to_bcp47() {
        assert_eq!(posix_to_bcp47("en_US.UTF-8").as_deref(), Some("en-US"));
        assert_eq!(posix_to_bcp47("de_DE@euro").as_deref(), Some("de-DE"));
        assert_eq!(posix_to_bcp47("C"), None);
        assert_eq!(posix_to_bcp47("POSIX"), None);
    }
}
