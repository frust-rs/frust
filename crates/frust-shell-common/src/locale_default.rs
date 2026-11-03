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
/// neither the idle write nor the drop write, so each shell calls [`Self::went_to_background`] at
/// its lifecycle `Hidden` edge (iOS `didEnterBackground`, Android `onStop`, a desktop whose every
/// window is minimized or occluded, or `suspended`) and [`Self::came_to_foreground`] at the
/// matching shown edge. An inactive-only transition (iOS resign-active, Android `onPause`, a
/// desktop focus loss) flushes nothing: the app is still on screen and may come straight back.
/// The flush is the existing contract: it writes, synchronously, when this cache or the shared
/// copy it is attached to holds answers not yet on disk. All contexts of a process share one cache
/// copy, so one call per context is bounded by one small file write. A flush through any one
/// context writes every answer the process holds that is not yet on disk, whichever context
/// recorded it.
///
/// The once-per-transition latch: a shell may report the same `Hidden` edge more than once (a
/// desktop re-derives its lifecycle on every focus, occlusion and suspend event, so a minimized
/// window that then loses focus reports `Hidden` twice), and a repeat before the foreground is
/// ignored, so one transition costs at most one write.
///
/// The retry rule: the latch arms only when the transition wrote something, that is when at least
/// one context flushed cleanly (or there was none to flush). When every flush failed the latch
/// stays unarmed, so the next background report of the same transition tries again instead of
/// being ignored; nothing retries on its own. A failed write is logged at warn the first time this
/// hook sees one and at debug after that, so a disk that stays full does not warn on every
/// background.
///
/// Interior-atomic and `const`-constructible so a shell without a spare handle field can keep it in a
/// `static`.
#[derive(Debug)]
pub struct BackgroundFlush {
    backgrounded: AtomicBool,
    /// Set by the first failed flush, which logs at warn; later failures log at debug.
    warned: AtomicBool,
}

/// What one [`BackgroundFlush::went_to_background_outcome`] call did: a repeat of a transition
/// already flushed, or one result per context in call order (`Err` carries the failed write's
/// [`io::ErrorKind`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundFlushOutcome {
    repeat: bool,
    contexts: Vec<Result<(), io::ErrorKind>>,
}

impl BackgroundFlushOutcome {
    /// Whether the app was already backgrounded, so nothing was flushed.
    pub fn is_repeat(&self) -> bool {
        self.repeat
    }

    /// One result per context, in the order the shell passed them; empty for a repeat.
    pub fn contexts(&self) -> &[Result<(), io::ErrorKind>] {
        &self.contexts
    }

    /// How many contexts flushed cleanly.
    pub fn flushed(&self) -> usize {
        self.contexts.iter().filter(|r| r.is_ok()).count()
    }

    /// How many contexts' flushes failed.
    pub fn failed(&self) -> usize {
        self.contexts.len() - self.flushed()
    }

    /// Whether the latch was left unarmed because every flush failed, so the next
    /// [`BackgroundFlush::went_to_background`] flushes again.
    pub fn will_retry(&self) -> bool {
        !self.repeat && !self.contexts.is_empty() && self.flushed() == 0
    }
}

impl BackgroundFlush {
    /// A hook in the foreground state.
    pub const fn new() -> Self {
        Self {
            backgrounded: AtomicBool::new(false),
            warned: AtomicBool::new(false),
        }
    }

    /// [`Self::went_to_background_outcome`], reduced to how many contexts flushed cleanly (0 for
    /// a repeat).
    pub fn went_to_background(&self, contexts: &mut [&mut dyn FontCacheFlush]) -> usize {
        self.went_to_background_outcome(contexts).flushed()
    }

    /// Flushes every context once if this is a new background transition, and reports each
    /// result. A failed write is logged (warn the first time, debug after that); the cache stays
    /// dirty, so the drop write remains.
    /// When every flush failed the latch is left unarmed and the next call retries (the type's
    /// retry rule); otherwise a repeat before [`Self::came_to_foreground`] flushes nothing.
    pub fn went_to_background_outcome(
        &self,
        contexts: &mut [&mut dyn FontCacheFlush],
    ) -> BackgroundFlushOutcome {
        if self.backgrounded.swap(true, Ordering::AcqRel) {
            return BackgroundFlushOutcome {
                repeat: true,
                contexts: Vec::new(),
            };
        }
        let results = contexts
            .iter_mut()
            .map(|cx| {
                cx.flush_font_cache().map_err(|err| {
                    if self.warned.swap(true, Ordering::AcqRel) {
                        log::debug!("frust-text: font cache flush failed: {err}");
                    } else {
                        log::warn!("frust-text: font cache flush failed: {err}");
                    }
                    err.kind()
                })
            })
            .collect();
        let outcome = BackgroundFlushOutcome {
            repeat: false,
            contexts: results,
        };
        if outcome.will_retry() {
            // A foreground in between stores `false` too, so this cannot lose a re-arm.
            self.backgrounded.store(false, Ordering::Release);
            log::info!("frust-text: font cache flush failed; the next background retries");
        } else {
            log::info!(
                "frust-text: font cache flushed ({} contexts)",
                outcome.flushed()
            );
        }
        outcome
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
        // A second report of the same hide (a repeated `Hidden` edge) flushes nothing.
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
    fn a_failing_flush_leaves_the_latch_unarmed_and_retries() {
        let hook = BackgroundFlush::new();
        let mut bad = Counting {
            flushes: 0,
            fail: true,
        };
        let outcome = hook.went_to_background_outcome(&mut [&mut bad]);
        assert_eq!(outcome.contexts(), &[Err(io::ErrorKind::Other)]);
        assert!(outcome.will_retry() && !outcome.is_repeat());
        assert_eq!((outcome.flushed(), outcome.failed()), (0, 1));
        // The same transition reported again (a repeated `Hidden` edge) tries again.
        assert_eq!(hook.went_to_background(&mut [&mut bad]), 0);
        assert_eq!(bad.flushes, 2);
        // Once the disk recovers, the retry arms the latch.
        bad.fail = false;
        let outcome = hook.went_to_background_outcome(&mut [&mut bad]);
        assert_eq!(outcome.contexts(), &[Ok(())]);
        assert!(!outcome.will_retry());
        assert_eq!(bad.flushes, 3);
        let repeat = hook.went_to_background_outcome(&mut [&mut bad]);
        assert!(repeat.is_repeat() && repeat.contexts().is_empty() && !repeat.will_retry());
        assert_eq!(bad.flushes, 3);
    }

    #[test]
    fn only_the_first_failure_warns() {
        let hook = BackgroundFlush::new();
        let mut good = counting();
        hook.went_to_background(&mut [&mut good]);
        assert!(
            !hook.warned.load(Ordering::Acquire),
            "a clean flush warns nothing"
        );
        hook.came_to_foreground();
        let mut bad = Counting {
            flushes: 0,
            fail: true,
        };
        hook.went_to_background(&mut [&mut bad]);
        assert!(
            hook.warned.load(Ordering::Acquire),
            "the first failure warns"
        );
        // Later failures (the retry, the next transition) take the debug branch; the latch rule
        // is unchanged by the logging.
        assert!(
            hook.went_to_background_outcome(&mut [&mut bad])
                .will_retry()
        );
        assert_eq!(bad.flushes, 2);
    }

    #[test]
    fn a_succeeding_flush_arms_the_latch() {
        let hook = BackgroundFlush::new();
        let mut good = counting();
        let outcome = hook.went_to_background_outcome(&mut [&mut good]);
        assert_eq!(outcome.contexts(), &[Ok(())]);
        assert!(!outcome.will_retry());
        assert!(
            hook.went_to_background_outcome(&mut [&mut good])
                .is_repeat()
        );
        assert_eq!(good.flushes, 1);
    }

    #[test]
    fn a_partial_failure_arms_the_latch() {
        let hook = BackgroundFlush::new();
        let mut bad = Counting {
            flushes: 0,
            fail: true,
        };
        let mut good = counting();
        let outcome = hook.went_to_background_outcome(&mut [&mut bad, &mut good]);
        assert_eq!(outcome.contexts(), &[Err(io::ErrorKind::Other), Ok(())]);
        assert!(!outcome.will_retry());
        assert_eq!(hook.went_to_background(&mut [&mut bad, &mut good]), 0);
        assert_eq!((bad.flushes, good.flushes), (1, 1));
    }

    #[test]
    fn no_contexts_arms_the_latch() {
        let hook = BackgroundFlush::new();
        let outcome = hook.went_to_background_outcome(&mut []);
        assert!(!outcome.will_retry() && !outcome.is_repeat());
        assert!(hook.went_to_background_outcome(&mut []).is_repeat());
    }

    #[test]
    fn a_real_text_context_flushes() {
        let hook = BackgroundFlush::new();
        let mut cx = TextContext::builder().without_font_cache().build();
        assert_eq!(hook.went_to_background(&mut [&mut cx]), 1);
    }
}
