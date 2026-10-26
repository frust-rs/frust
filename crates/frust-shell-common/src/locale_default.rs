//! Per-frame detection of a change to the app's default locale.
//!
//! The default locale ([`frust_text::set_default_locale`]) picks the face for untagged text
//! (a Han string under `ja` versus `zh`), and `Text`/`TextInput` fold its generation into their
//! layout cache. A change needs no rebuild, only a layout pass, which a shell forces the way it
//! does for a late font registration: re-push the active theme (`LAYOUT | PAINT`). This watcher is
//! the shell-instance half, remembering the last generation seen; poll it once per frame beside
//! the theme-override poll.

use frust_text::default_locale_generation;

/// Remembers the default-locale generation a shell last acted on.
#[derive(Debug)]
pub struct LocaleDefaultWatcher {
    last_generation: u64,
}

impl LocaleDefaultWatcher {
    /// A watcher at the slot's current generation, so a locale set before the shell started
    /// (the first layout already reads it) does not force a redundant relayout.
    pub fn new() -> Self {
        Self {
            last_generation: default_locale_generation(),
        }
    }

    /// Whether the default locale changed since the previous call (or construction).
    pub fn changed(&mut self) -> bool {
        let now = default_locale_generation();
        if now == self.last_generation {
            return false;
        }
        self.last_generation = now;
        true
    }
}

impl Default for LocaleDefaultWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_change_once() {
        // The slot is process-global; compare generations relative to this watcher only.
        let mut watcher = LocaleDefaultWatcher::new();
        assert!(!watcher.changed());
        let tag = format!("x-watch-{}", default_locale_generation());
        frust_text::set_default_locale(Some(&tag));
        assert!(watcher.changed());
        assert!(!watcher.changed());
        frust_text::set_default_locale(None);
        assert!(watcher.changed());
    }
}
