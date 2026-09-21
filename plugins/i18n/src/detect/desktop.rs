//! The desktop detection backend — `sys-locale`, covering every target that
//! is neither Android nor iOS (macOS included: `NSLocale` lives in
//! [`crate::detect::apple`], which is iOS-only, per this crate's `Cargo.toml`
//! design note).
//!
//! `sys_locale::get_locales()` is BCP-47-normalized on every arm it
//! supports for a real language preference — it converts POSIX-style tags
//! (`en_US.UTF-8`) itself — but on a POSIX host whose only configured
//! locale is `C`/`POSIX` (no language to report at all, e.g. a bare
//! `LANG=C.UTF-8` container) it hands that sentinel back verbatim instead
//! of an empty list, so this backend runs every raw tag through
//! [`posix_to_bcp47`] itself before returning it — the same "`C`/`POSIX`
//! carries no usable language" rule the env-var fallback below has always
//! applied, now held for every source, not just that last one. Its own
//! crate doc still allows for a completely empty result on a
//! platform/environment it has no reader for (a minimal container with no
//! locale subsystem, for instance), so this backend falls back, in order:
//! `sys_locale::get_locales()` (the full preference list) →
//! `sys_locale::get_locale()` (its own single-value convenience, kept as
//! an explicit fallback per this crate's design even though it is
//! `get_locales().next()` under the hood) → a last-resort `LANG`-style
//! POSIX env-var read. A `C`/`POSIX`-only host legitimately falls through
//! all three and yields an empty `Vec` — that is not a bug; see
//! [`super::system_locales`]'s documented [`crate::I18nError::Detection`].

/// `sys_locale::get_locales()`, falling back to `sys_locale::get_locale()`
/// then a `LANG`-style env read only when the list comes back empty (see
/// the module doc). Every candidate tag, from every source, is run through
/// [`posix_to_bcp47`] so a `C`/`POSIX` sentinel never survives to the
/// caller.
pub(crate) fn raw_locale_tags() -> Vec<String> {
    let locales: Vec<String> = sys_locale::get_locales()
        .filter_map(|tag| posix_to_bcp47(&tag))
        .collect();
    if !locales.is_empty() {
        return locales;
    }
    if let Some(locale) = sys_locale::get_locale().and_then(|tag| posix_to_bcp47(&tag)) {
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

    /// Every tag `raw_locale_tags()` returns must parse as BCP-47 — this is
    /// the property that actually matters (a `C`/`POSIX`-only host, e.g.
    /// `LANG=C.UTF-8` with every other locale variable unset, legitimately
    /// yields an *empty* list per the module doc, so non-emptiness is not
    /// asserted here). This still catches a real regression: if
    /// `posix_to_bcp47` — or the filtering this function now runs every
    /// raw tag through — ever let a `C`/`POSIX` sentinel or other garbage
    /// through unfiltered, the loop below would find it and fail to parse
    /// it as a [`crate::Locale`], exactly as it did before this fix.
    #[test]
    fn desktop_locales_are_parseable_when_any_are_reported() {
        let tags = raw_locale_tags();
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
