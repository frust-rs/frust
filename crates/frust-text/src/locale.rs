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

/// Serialises the tests that write the process-global slot.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());

    /// Holds the test lock and restores the value found at `set` on drop.
    pub(crate) struct DefaultLocale(#[allow(dead_code)] MutexGuard<'static, ()>, Option<String>);

    impl DefaultLocale {
        /// Takes the lock and sets the default locale to `locale`.
        pub(crate) fn set(locale: Option<&str>) -> Self {
            let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = super::default_locale().map(|tag| tag.to_string());
            super::set_default_locale(locale);
            Self(guard, previous)
        }
    }

    impl Drop for DefaultLocale {
        fn drop(&mut self) {
            super::set_default_locale(self.1.as_deref());
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
}
