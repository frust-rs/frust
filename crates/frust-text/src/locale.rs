//! The app's default locale: the language of text whose style carries no locale.
//!
//! A process-wide slot, written by the i18n plugin (or the app) when the active locale changes
//! and read by the font-fallback oracle and the shape cache. It is a default beneath the script
//! hints, not a fill of [`crate::TextStyle::locale`]: an explicit style locale wins, then the
//! language the cluster's script implies (kana `ja`, Hangul `ko`, ...), then the Han language
//! this locale names, then the platform's preference list (see the oracle's `language_for`).

use std::sync::{Arc, RwLock};

struct Slot {
    tag: Option<Arc<str>>,
    generation: u64,
}

static SLOT: RwLock<Slot> = RwLock::new(Slot {
    tag: None,
    generation: 0,
});

/// Sets the app's default locale (a BCP 47 tag such as `ja` or `zh-Hant-TW`); `None`, or a tag
/// that is empty once trimmed, clears it. The generation advances only when the value changes.
/// The slot is read on the thread that shapes, so writers (the i18n plugin, the app) call this on
/// that same thread (the UI thread). A write from another thread may pair one shaped paragraph or
/// persisted answer with the previous locale's key.
pub fn set_default_locale(locale: Option<&str>) {
    let tag = locale.map(str::trim).filter(|t| !t.is_empty());
    let mut slot = SLOT.write().unwrap_or_else(|e| e.into_inner());
    if slot.tag.as_deref() == tag {
        return;
    }
    slot.tag = tag.map(Arc::from);
    slot.generation += 1;
}

/// The app's default locale, `None` when none is set.
pub fn default_locale() -> Option<Arc<str>> {
    SLOT.read().unwrap_or_else(|e| e.into_inner()).tag.clone()
}

/// A counter that advances each time [`set_default_locale`] changes the value.
pub fn default_locale_generation() -> u64 {
    SLOT.read().unwrap_or_else(|e| e.into_inner()).generation
}

/// A cached layout's dependency on the default locale: the current [`default_locale_generation`]
/// for an untagged style, a constant for one carrying its own locale (the default never touches
/// it). Store it beside the style key; reuse the layout while [`is_current`](Self::is_current).
///
/// **Read the stamp before shaping** and store that value: a read after shaping can postdate a
/// locale write that landed mid-shape, pairing the old locale's layout with the new generation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LocaleStamp(u64);

impl LocaleStamp {
    /// The stamp for a style whose [`crate::TextStyle::locale`] is `explicit_locale`: a constant
    /// when it names one, the current generation otherwise.
    pub fn for_style(explicit_locale: Option<&str>) -> Self {
        if explicit_locale.is_some() {
            Self(0)
        } else {
            Self(default_locale_generation())
        }
    }

    /// Whether a layout stamped `self` is valid: always if tagged, else at the same generation.
    pub fn is_current(self, explicit_locale: Option<&str>) -> bool {
        explicit_locale.is_some() || self.0 == default_locale_generation()
    }
}

/// The lock over the process-global slot, compiled into this crate's tests and, behind the
/// `test-support` feature, the library: every crate in one test binary shares it.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());

    #[cfg(test)]
    pub(crate) type DefaultLocale = LocaleSlotGuard;

    /// Holds the slot lock; its drop (a panic included) restores the value found at the lock.
    pub struct LocaleSlotGuard {
        previous: Option<String>,
        _lock: MutexGuard<'static, ()>,
    }

    impl LocaleSlotGuard {
        /// Takes the lock and sets the default locale to `locale`.
        pub fn set(locale: Option<&str>) -> Self {
            let guard = Self::hold();
            super::set_default_locale(locale);
            guard
        }

        /// Takes the lock without writing the slot (a test that writes it later, or only reads it).
        pub fn hold() -> Self {
            let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = super::default_locale().map(|tag| tag.to_string());
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for LocaleSlotGuard {
        fn drop(&mut self) {
            super::set_default_locale(self.previous.as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::DefaultLocale;
    use super::*;

    #[test]
    fn set_get_and_clear() {
        let _guard = DefaultLocale::set(None);
        assert_eq!(default_locale(), None);
        set_default_locale(Some("ja"));
        assert_eq!(default_locale().as_deref(), Some("ja"));
        set_default_locale(None);
        assert_eq!(default_locale(), None);
    }

    #[test]
    fn the_tag_is_trimmed_and_blank_clears() {
        let _guard = DefaultLocale::set(None);
        set_default_locale(Some("  zh-Hant-TW \n"));
        assert_eq!(default_locale().as_deref(), Some("zh-Hant-TW"));
        set_default_locale(Some("   "));
        assert_eq!(default_locale(), None);
    }

    #[test]
    fn the_generation_advances_only_on_a_change() {
        let _guard = DefaultLocale::set(None);
        let start = default_locale_generation();
        set_default_locale(None);
        assert_eq!(default_locale_generation(), start);
        set_default_locale(Some("ja"));
        assert_eq!(default_locale_generation(), start + 1);
        set_default_locale(Some(" ja "));
        assert_eq!(default_locale_generation(), start + 1, "same value");
        set_default_locale(Some("ko"));
        assert_eq!(default_locale_generation(), start + 2);
        set_default_locale(None);
        assert_eq!(default_locale_generation(), start + 3);
    }

    #[test]
    fn an_explicit_locale_stamps_a_constant_that_is_always_current() {
        let _guard = DefaultLocale::set(None);
        let stamp = LocaleStamp::for_style(Some("ja"));
        set_default_locale(Some("ko"));
        assert_eq!(LocaleStamp::for_style(Some("ja")), stamp);
        assert_eq!(LocaleStamp::for_style(Some("zh-Hant")), stamp);
        assert!(stamp.is_current(Some("ja")));
        set_default_locale(None);
        assert!(stamp.is_current(Some("ja")), "the default never touches it");
    }

    #[test]
    fn a_generation_bump_makes_an_untagged_stamp_stale() {
        let _guard = DefaultLocale::set(None);
        let stamp = LocaleStamp::for_style(None);
        assert!(stamp.is_current(None));
        set_default_locale(None);
        assert!(stamp.is_current(None), "an unchanged value keeps it");
        set_default_locale(Some("ja"));
        assert!(!stamp.is_current(None), "stale after the change");
        assert!(LocaleStamp::for_style(None).is_current(None));
    }

    #[test]
    fn a_stamp_read_before_shaping_catches_a_write_during_the_shape() {
        let _guard = DefaultLocale::set(None);
        // The order a cache follows: stamp, shape, store the stamp with the layout.
        let before_shape = LocaleStamp::for_style(None);
        // A locale write lands while the layout is being shaped under the old default.
        set_default_locale(Some("ja"));
        let after_shape = LocaleStamp::for_style(None);
        assert!(
            !before_shape.is_current(None),
            "the stored stamp marks the layout stale, so the next pass re-shapes"
        );
        assert!(
            after_shape.is_current(None),
            "a stamp read after shaping would hide the stale layout"
        );
    }

    #[test]
    fn the_guard_restores_the_value_found_when_it_took_the_lock() {
        let probe = test_support::LocaleSlotGuard::hold();
        let steady = default_locale();
        drop(probe);
        let guard = test_support::LocaleSlotGuard::set(Some("ja"));
        assert_eq!(default_locale().as_deref(), Some("ja"));
        set_default_locale(Some("ko"));
        drop(guard);
        let _probe = test_support::LocaleSlotGuard::hold();
        assert_eq!(default_locale(), steady);
    }
}
