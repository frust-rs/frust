//! The [`Locale`] → ICU4X bridge and the shared formatter cache every
//! `fmt` submodule keys off it.
//!
//! # Why a bridge at all
//!
//! [`Locale`] wraps a `unic_langid::LanguageIdentifier` (the type
//! `fluent-bundle`/`fluent-langneg` speak); every ICU4X formatter is built
//! from an `icu_locale_core::LanguageIdentifier` instead. The two crates
//! model the same BCP-47 grammar but share no type, so the crossing is a
//! parse of the canonical tag [`Locale`] `Display`s — cheap, but not free,
//! and on a path a UI can hit per frame, so it is memoized per locale.
//!
//! An unparseable tag is **not** an error: it degrades to the root (`und`)
//! locale with a warn-level log, so a formatting call still returns text.
//! The only way to reach that arm is a tag `unic_langid` accepts and
//! `icu_locale_core` rejects — no such tag is known, which is exactly why
//! this is a logged degrade rather than a variant on [`crate::I18nError`].
//!
//! # [`Cache`]: build once per key, format without a lock
//!
//! Building an ICU4X formatter loads and parses baked CLDR data; formatting
//! with one is pure. [`Cache`] keeps built formatters behind an `RwLock`,
//! but hands out `Arc` clones — a caller takes the read lock only long
//! enough to clone the pointer, so **no lock is held while formatting**, and
//! a slow formatting call on one thread cannot stall another's cache lookup.
//!
//! Two threads racing the same cold key both build; the second insert
//! replaces the first and both callers keep a working formatter. A duplicated
//! build is wasted work, never a wrong answer, which is why this is not
//! serialized behind a write lock for the whole build.
//!
//! Both locks recover from poisoning (`PoisonError::into_inner`) instead of
//! unwrapping: a panic elsewhere must not turn every later formatting call
//! into a panic of its own, and nothing this module stores can be left
//! half-written by one (an insert is a whole `Arc`).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use icu_locale_core::LanguageIdentifier;

use crate::Locale;

/// Memoized [`Locale`] → ICU4X identifier crossings.
static LANG_IDS: LazyLock<RwLock<HashMap<Locale, LanguageIdentifier>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The ICU4X language identifier for `locale`, parsed once per locale.
pub(super) fn lang_id(locale: &Locale) -> LanguageIdentifier {
    if let Some(found) = LANG_IDS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(locale)
    {
        return found.clone();
    }

    let parsed = parse(locale);
    LANG_IDS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(locale.clone(), parsed.clone());
    parsed
}

/// Parses `locale`'s canonical BCP-47 tag into ICU4X's identifier, degrading
/// to the root locale (with a log) rather than failing.
fn parse(locale: &Locale) -> LanguageIdentifier {
    let tag = locale.to_string();
    match tag.parse::<LanguageIdentifier>() {
        Ok(id) => id,
        Err(error) => {
            log::warn!(
                "frust-i18n: `{tag}` is not an ICU4X language identifier ({error}) \
                 — formatting it falls back to the root locale"
            );
            LanguageIdentifier::UNKNOWN
        }
    }
}

/// A build-once map of constructed ICU4X formatters.
///
/// `K` is whatever fully determines a formatter — a locale, or a locale
/// paired with a length — and `V` the formatter itself. See this module's
/// header for the locking contract.
pub(super) struct Cache<K, V> {
    entries: RwLock<HashMap<K, Arc<V>>>,
}

impl<K: Clone + Eq + Hash, V> Cache<K, V> {
    /// An empty cache.
    ///
    /// Not `const` (`HashMap::new` isn't either), so every user wraps this in
    /// a `LazyLock` rather than a bare `static`.
    pub(super) fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// The formatter for `key`, building it through `build` on a miss.
    ///
    /// A failed build is returned to the caller and nothing is cached, so a
    /// later call retries rather than memoizing the failure.
    pub(super) fn get_or_build<E>(
        &self,
        key: &K,
        build: impl FnOnce() -> Result<V, E>,
    ) -> Result<Arc<V>, E> {
        if let Some(found) = self
            .entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Ok(Arc::clone(found));
        }

        let built = Arc::new(build()?);
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), Arc::clone(&built));
        Ok(built)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    #[test]
    fn a_locale_crosses_to_the_same_icu_identifier_every_time() {
        let first = lang_id(&locale("de-CH"));
        let second = lang_id(&locale("de-CH"));

        assert_eq!(first, second);
        assert_eq!(first.to_string(), "de-CH");
    }

    #[test]
    fn script_and_region_survive_the_crossing() {
        assert_eq!(lang_id(&locale("zh-Hans-CN")).to_string(), "zh-Hans-CN");
    }

    #[test]
    fn a_cache_builds_once_per_key() {
        let cache: Cache<u8, u8> = Cache::new();
        let mut builds = 0;

        for _ in 0..3 {
            let value = cache
                .get_or_build::<()>(&1, || {
                    builds += 1;
                    Ok(7)
                })
                .expect("builds");
            assert_eq!(*value, 7);
        }

        assert_eq!(builds, 1);
    }

    #[test]
    fn a_failed_build_is_not_cached() {
        let cache: Cache<u8, u8> = Cache::new();

        assert!(cache.get_or_build(&1, || Err("nope")).is_err());
        assert_eq!(*cache.get_or_build::<()>(&1, || Ok(7)).expect("retries"), 7);
    }
}
