//! Per-frame detection of a change to the app's default locale.
//!
//! The default locale ([`frust_text::set_default_locale`]) picks the face for untagged text
//! (a Han string under `ja` versus `zh`), and `Text`/`TextInput` fold its generation into their
//! layout cache. A change needs no rebuild, only a layout pass, which a shell forces the way it
//! does for a late font registration: re-push the active theme (`LAYOUT | PAINT`). This watcher is
//! the shell-instance half, remembering the last generation seen; poll it once per frame beside
//! the theme-override poll.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use frust_text::{TextContext, default_locale_generation};

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

/// What the background hook flushes: a text context's persistent font cache.
///
/// [`TextContext`] is the only type that implements it in the tree; the trait exists so the hook's
/// once-per-transition rule is testable with a counting stand-in.
pub trait FontCacheFlush {
    /// Writes unwritten font-oracle answers now ([`TextContext::flush_font_cache`]).
    fn flush_font_cache(&mut self) -> io::Result<()>;
}

impl FontCacheFlush for TextContext {
    fn flush_font_cache(&mut self) -> io::Result<()> {
        TextContext::flush_font_cache(self)
    }
}

/// The shells' shared "app went to the background" font-cache hook.
///
/// The oracle cache writes behind on its own, but a mobile process killed in the background gets
/// neither the idle write nor the drop write, so each shell calls [`Self::went_to_background`] from
/// its pause/background point and [`Self::came_to_foreground`] from the matching resume. Platforms
/// that report a background in several steps (Android `onPause` then `onStop`, iOS resign then
/// enter-background, a desktop focus loss then `suspended`) flush once: a repeat before the
/// foreground is ignored. The flush is the existing contract (a clean cache is a no-op; a dirty one
/// writes synchronously), and all contexts of a process share one cache copy, so one call per
/// context is bounded by one small file write. A flush through any one context writes every
/// answer the process holds that is not yet on disk, whichever context recorded it.
///
/// Interior-atomic and `const`-constructible so a shell without a spare handle field can keep it in a
/// `static`.
#[derive(Debug)]
pub struct BackgroundFlush {
    backgrounded: AtomicBool,
}

impl BackgroundFlush {
    /// A hook in the foreground state.
    pub const fn new() -> Self {
        Self {
            backgrounded: AtomicBool::new(false),
        }
    }

    /// Flushes every context once if this is a new background transition; returns how many
    /// flushed cleanly (0 when the app was already backgrounded). A failed write is logged at warn
    /// and not retried until the next transition: the cache stays dirty and the drop write remains.
    pub fn went_to_background(&self, contexts: &mut [&mut dyn FontCacheFlush]) -> usize {
        if self.backgrounded.swap(true, Ordering::AcqRel) {
            return 0;
        }
        let mut flushed = 0;
        for cx in contexts.iter_mut() {
            match cx.flush_font_cache() {
                Ok(()) => flushed += 1,
                Err(err) => log::warn!("frust-text: font cache flush failed: {err}"),
            }
        }
        log::info!("frust-text: font cache flushed ({flushed} contexts)");
        flushed
    }

    /// Re-arms the hook for the next background transition. Flushes nothing.
    pub fn came_to_foreground(&self) {
        self.backgrounded.store(false, Ordering::Release);
    }
}

impl Default for BackgroundFlush {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Restores the process-global slot to unset, panic included.
    struct Restore;

    impl Drop for Restore {
        fn drop(&mut self) {
            frust_text::set_default_locale(None);
        }
    }

    #[test]
    fn reports_a_change_once() {
        let _restore = Restore;
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

    struct Counting {
        flushes: usize,
        fail: bool,
    }

    impl FontCacheFlush for Counting {
        fn flush_font_cache(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.fail {
                Err(io::Error::other("disk full"))
            } else {
                Ok(())
            }
        }
    }

    fn counting() -> Counting {
        Counting {
            flushes: 0,
            fail: false,
        }
    }

    #[test]
    fn background_flushes_every_context_once_per_transition() {
        let hook = BackgroundFlush::new();
        let (mut a, mut b) = (counting(), counting());
        assert_eq!(hook.went_to_background(&mut [&mut a, &mut b]), 2);
        assert_eq!((a.flushes, b.flushes), (1, 1));
        // A second report of the same background (onPause then onStop) flushes nothing.
        assert_eq!(hook.went_to_background(&mut [&mut a, &mut b]), 0);
        assert_eq!((a.flushes, b.flushes), (1, 1));
        // The foreground flushes nothing, and re-arms the next transition.
        hook.came_to_foreground();
        assert_eq!((a.flushes, b.flushes), (1, 1));
        assert_eq!(hook.went_to_background(&mut [&mut a, &mut b]), 2);
        assert_eq!((a.flushes, b.flushes), (2, 2));
    }

    #[test]
    fn foreground_without_a_background_flushes_nothing() {
        let hook = BackgroundFlush::new();
        let mut a = counting();
        hook.came_to_foreground();
        assert_eq!(a.flushes, 0);
        assert_eq!(hook.went_to_background(&mut [&mut a]), 1);
    }

    #[test]
    fn a_failed_flush_does_not_stop_the_others() {
        let hook = BackgroundFlush::new();
        let mut bad = Counting {
            flushes: 0,
            fail: true,
        };
        let mut good = counting();
        assert_eq!(hook.went_to_background(&mut [&mut bad, &mut good]), 1);
        assert_eq!((bad.flushes, good.flushes), (1, 1));
    }

    #[test]
    fn a_real_text_context_flushes() {
        let hook = BackgroundFlush::new();
        let mut cx = TextContext::builder().without_font_cache().build();
        assert_eq!(hook.went_to_background(&mut [&mut cx]), 1);
    }
}
