//! Locale-override persistence (feature `persistence`): the saved choice is
//! restored when an [`I18n`] is built and rewritten by [`I18n::set_locale`].

use frust_shared_preferences::SharedPreferences;

use crate::{I18n, I18nError, Locale, LocaleSet};

/// The preferences key the chosen locale is stored under.
pub const LOCALE_KEY: &str = "frust_i18n.locale";

/// The locale saved under [`LOCALE_KEY`], if one is stored and still parses.
pub fn persisted_locale(prefs: &SharedPreferences) -> Option<Locale> {
    prefs.get_string(LOCALE_KEY)?.parse().ok()
}

/// Best-effort save: a failure loses only the next launch's default, never
/// the already-switched UI, so it is logged rather than propagated.
pub(crate) fn save(prefs: &SharedPreferences, locale: &Locale) {
    if let Err(error) = prefs.set_string(LOCALE_KEY, locale.to_string()) {
        log::warn!("frust-i18n: could not persist locale `{locale}`: {error}");
    }
}

impl I18n {
    /// Like [`I18n::new`], but a locale previously saved in `prefs` wins:
    /// it is prepended to `requested`, so negotiation prefers it over the
    /// system list. The handle keeps `prefs`, and every later
    /// [`I18n::set_locale`] saves the chosen tag back (failures are logged,
    /// never fatal).
    ///
    /// # Errors
    ///
    /// The same contract as [`I18n::new`].
    pub fn with_persistence(
        locales: LocaleSet,
        requested: &[Locale],
        prefs: SharedPreferences,
    ) -> Result<Self, I18nError> {
        let engine = crate::Engine::new(locales)?;
        let mut list: Vec<Locale> = persisted_locale(&prefs).into_iter().collect();
        list.extend_from_slice(requested);
        Ok(Self::persistent(engine, &list, prefs))
    }

    /// Removes the saved locale so the next launch starts from the system
    /// list again. A no-op on a handle built without persistence; failures
    /// are logged.
    pub fn clear_persisted_locale(&self) {
        if let Some(prefs) = self.prefs()
            && let Err(error) = prefs.remove(LOCALE_KEY)
        {
            log::warn!("frust-i18n: could not clear the persisted locale: {error}");
        }
    }
}
