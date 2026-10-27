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
//! The cache is bounded ([`DEFAULT_CAPACITY`], least recently used evicted
//! first) so a caller cycling through arbitrary key combinations cannot grow
//! it without limit; an evicted formatter is rebuilt on its next use. The
//! locale memo above holds one small identifier per distinct locale an app
//! ever formats with, which is bounded by the locales it ships and negotiates.
//!
//! Both locks recover from poisoning (`PoisonError::into_inner`) instead of
//! unwrapping: a panic elsewhere must not turn every later formatting call
//! into a panic of its own, and nothing this module stores can be left
//! half-written by one (an insert is a whole `Arc`).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// The most entries a [`Cache::new`] cache keeps before it evicts.
///
/// Generous next to what an app uses (a handful of locales times a few
/// lengths and styles), small enough that a caller cycling through arbitrary
/// keys cannot grow a cache without limit.
pub(super) const DEFAULT_CAPACITY: usize = 256;

/// A bounded, build-once map of constructed ICU4X formatters.
///
/// `K` is whatever fully determines a formatter — a locale, or a locale
/// paired with a length — and `V` the formatter itself. See this module's
/// header for the locking contract.
///
/// At most `capacity` entries are kept; inserting past that evicts the least
/// recently used one, which is simply rebuilt on its next use. Recency is an
/// atomic tick stamped on every hit, so a hit still takes only the read lock.
pub(super) struct Cache<K, V> {
    capacity: usize,
    clock: AtomicU64,
    entries: RwLock<HashMap<K, Entry<V>>>,
}

struct Entry<V> {
    value: Arc<V>,
    last_used: AtomicU64,
}

impl<K: Clone + Eq + Hash, V> Cache<K, V> {
    /// An empty cache holding at most [`DEFAULT_CAPACITY`] entries.
    ///
    /// Not `const` (`HashMap::new` isn't either), so every user wraps this in
    /// a `LazyLock` rather than a bare `static`.
    pub(super) fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// An empty cache holding at most `capacity` entries (at least one).
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            clock: AtomicU64::new(0),
            entries: RwLock::new(HashMap::new()),
        }
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
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
            found.last_used.store(self.tick(), Ordering::Relaxed);
            return Ok(Arc::clone(&found.value));
        }

        let built = Arc::new(build()?);
        let mut entries = self.entries.write().unwrap_or_else(PoisonError::into_inner);
        entries.insert(
            key.clone(),
            Entry {
                value: Arc::clone(&built),
                last_used: AtomicU64::new(self.tick()),
            },
        );
        if entries.len() > self.capacity {
            let oldest = entries
                .iter()
                .filter(|(held, _)| *held != key)
                .min_by_key(|(_, entry)| entry.last_used.load(Ordering::Relaxed))
                .map(|(held, _)| held.clone());
            if let Some(oldest) = oldest {
                entries.remove(&oldest);
            }
        }
        Ok(built)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
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

    #[test]
    fn a_cache_is_bounded_and_evicts_least_recently_used() {
        let cache: Cache<u32, u32> = Cache::with_capacity(3);
        let build = |value: u32| move || Ok::<_, ()>(value);

        for key in 0..3 {
            cache.get_or_build(&key, build(key)).unwrap();
        }
        // Touch 0 so 1 is the oldest, then overflow.
        cache.get_or_build(&0, build(99)).unwrap();
        cache.get_or_build(&3, build(3)).unwrap();

        assert_eq!(cache.len(), 3);
        assert_eq!(*cache.get_or_build(&0, build(99)).unwrap(), 0, "0 survived");
        assert_eq!(*cache.get_or_build(&1, build(11)).unwrap(), 11, "1 evicted");

        for key in 10..100 {
            cache.get_or_build(&key, build(key)).unwrap();
        }
        assert_eq!(cache.len(), 3);
    }
}
