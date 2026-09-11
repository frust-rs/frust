//! The AVFoundation backend — one `AVPlayer`/`AVPlayerItem` session per open,
//! driven **from Rust directly** via `objc2-av-foundation` (no Swift glue),
//! serving **iOS and macOS alike**: the module is gated `target_vendor =
//! "apple"` by [`crate`], so a native macOS host build compiles it too. Only
//! the hosting view differs per platform ([`crate::ios_view`] vs
//! [`crate::macos_view`]), and neither reaches the player over FFI — both call
//! [`player_for`].
//!
//! # Threading: everything Objective-C happens on the main thread
//!
//! `AVPlayer` is documented thread-safe, but this module deliberately confines
//! every message send, every object construction, and every release to the
//! platform **main thread** — reached through [`on_main`], which runs a body
//! inline when the caller is already there and `dispatch_async`es it onto
//! `dispatch_get_main_queue()` when it is not. Three things fall out of that,
//! and together they are the contract v3-02/v3-03 build on:
//!
//! - **Nothing blocks.** No path here ever waits on the main queue
//!   (`exec_async`, never `exec_sync`), so [`crate::VideoPlayer::open`] and
//!   every [`crate::PlayerSession`] control method keep the crate's
//!   non-blocking, UI-thread-safe promise even when called from a worker.
//! - **Every `Shared::publish` a listener can observe happens on the main
//!   thread**, which is exactly what the crate's listener contract promises
//!   an app ([`crate::PlayerSession::set_listener`]). The one publish made on
//!   the caller's thread is the initial `Loading` inside `open`, issued before
//!   the [`crate::PlayerSession`] — and so any listener — can exist; it only
//!   seeds the snapshot. Observation is delivered there
//!   already — `AVPlayer` serializes its KVO notifications onto the main queue
//!   by default, and both the periodic time observer and the seek completion
//!   handler are enqueued on it explicitly — and the notification path hops
//!   through [`on_main`] rather than trusting the posting thread.
//! - **`Retained<AVPlayer>` never crosses a thread.** The registry
//!   ([`SESSIONS`]) is a plain `Mutex<HashMap<..>>` whose Objective-C half sits
//!   behind [`MainThreadBound`], a wrapper whose `Send`/`Sync` assertions are
//!   discharged by that confinement (the `QueueBound` shape
//!   `plugins/camera/src/apple.rs` uses for its serial session queue).
//!
//! One rule keeps the registry lock honest: **no player message is ever sent,
//! and no event is ever published, while the lock is held.** A body takes what
//! it needs out of the map ([`live`], [`update_session`]), releases the lock,
//! and only then talks to the player — because a listener is allowed to close
//! its own session from inside an event ([`crate::PlayerSession::close`]), and
//! that close takes the same lock. The one Objective-C traffic the rule admits
//! is the `retain` a `Retained::clone` performs while lifting a handle out of
//! the map: a refcount bump runs no user code and so cannot re-enter here.
//!
//! # The accessor contract (frozen — v3-02 and v3-03 depend on it)
//!
//! [`player_for`] answers a live session's retained `AVPlayer` and `None` for
//! an unknown, not-yet-constructed, or closed id. It is callable on the main
//! thread (it answers `None` with a log line anywhere else — an
//! `AVPlayerLayer` could not be attached from another thread regardless), it
//! never blocks, and it is the **only** way either view factory reaches a
//! player. Nothing in this crate is exported over the C ABI.
//!
//! The spellings the factories parse are the crate root's, not this module's:
//! [`crate::VIEW_TYPE`] is `"VideoPlayerViewFactory"` on Apple and
//! [`crate::PlayerSession::params_json`] writes
//! `{"session":N,"fit":"contain"|"cover"}`. Neither literal is repeated here.
//!
//! # What v3-02/v3-03 can assume this module publishes
//!
//! The observation set is fixed, and every entry below lands on the main
//! thread as a [`crate::PlayerEvent`]:
//!
//! | Source | Key path / name | Publishes |
//! |---|---|---|
//! | KVO on the player | `currentItem.status` | `unknown` → `StateChanged(Loading)`; `readyToPlay` → `Position { position, duration }` then `StateChanged(Paused\|Playing\|Buffering)`; `failed` → `Error(..)` + `StateChanged(Error)` |
//! | KVO on the player | `timeControlStatus` | `paused` → `StateChanged(Paused)`, `playing` → `StateChanged(Playing)`, `waitingToPlayAtSpecifiedRate` → `StateChanged(Buffering)` |
//! | KVO on the player | `currentItem.presentationSize` | `VideoSize { width, height }` (a zero size is dropped, never published as `0x0`) |
//! | `NSNotificationCenter` | `AVPlayerItemDidPlayToEndTimeNotification` | `StateChanged(Ended)`, or a seek to zero + `play()` while looping |
//! | Periodic time observer | every 0.25 s, main queue | `Position { position, duration }` |
//! | Seek completion handler | once per [`crate::PlayerSession::seek_to`] | `Position { position, duration }` |
//! | [`AppleSession::close`] | — | `StateChanged(Idle)`, after the teardown |
//!
//! A factory needs none of it — [`player_for`] is its whole surface — but the
//! table is what a future observer of this module is held to.
//!
//! # Looping: `actionAtItemEnd`, not `AVPlayerLooper`
//!
//! A looping session sets `actionAtItemEnd = .none` and restarts itself from
//! the `AVPlayerItemDidPlayToEndTime` observer (seek to zero, then `play`);
//! a non-looping one sets `.pause` and reports [`crate::PlaybackState::Ended`].
//! Apple's own gapless recipe is `AVPlayerLooper` over an `AVQueuePlayer`, and
//! the seek-to-zero restart is a community pattern rather than a documented
//! one: it can show an audible gap at the loop point. Switching is a
//! **contained** change — `AVQueuePlayer` subclasses `AVPlayer`, so
//! [`player_for`] and both view factories are unaffected — and is the remedy
//! if the device gate hears a gap.
//!
//! # iOS audio-session policy
//!
//! On iOS **only** (macOS has no `AVAudioSession`; the asymmetry is the
//! platform's, and `docs/PLUGINS_CODE_STANDARDS.md` records a capability gap
//! per-platform rather than emulating it), the **first** open in the process
//! configures the shared session with category `.playback` — plus
//! `.mixWithOthers` when [`crate::PlayerOptions::mix_with_others`] asked for
//! it — and activates it, so playback is audible with the ringer switch
//! silenced. Later opens leave it alone: the category is process-wide state an
//! app may legitimately own, and re-asserting it on every open would stomp on
//! that. Every failure is logged and never fatal — a video that plays silently
//! is better than one that refuses to open.
//!
//! # Factory registration is lazy, from `open`
//!
//! [`AppleBackend::open`] calls its platform's `ensure_registered()`
//! ([`crate::ios_view`]/[`crate::macos_view`]) before anything else,
//! unconditionally and on the calling thread. Both are idempotent, so the cost
//! is one atomic after the first call, and the timing is what matters: a
//! `define_class!` factory must be realized before `NSClassFromString` can find
//! it, and a session always exists before the slot naming it can be created —
//! so registering from `open` is always before that slot's first Create.
//!
//! # `unsafe`
//!
//! Confined to this module and each `# Safety`-noted, the
//! `plugins/camera/src/apple.rs` precedent (`docs/CODE_STANDARDS.md`'s
//! sanctioned zones): `objc2` marks every AVFoundation message send `unsafe`,
//! edition-2024 marks reading an `extern` constant static `unsafe`, and
//! [`MainThreadBound`] carries one thread-safety assertion the compiler cannot
//! check.
//!
//! Nothing here unwinds into the Objective-C runtime: [`on_main`] wraps every
//! body it runs in `catch_unwind`, and the two blocks handed to AVFoundation
//! (the periodic time observer, the seek completion handler) reach their work
//! only through it. No path outside the unit tests calls `unwrap`/`expect`; a
//! poisoned registry lock is recovered from rather than propagated
//! ([`sessions`]).

// Under `cfg(test)` `crate::backend::Active` selects the in-memory mock on
// every target, Apple included, so the backend and session types here have no
// caller in a test build — the suite drives the fake by design (see
// `crate::backend::Active`'s own doc).
#![cfg_attr(test, allow(dead_code))]

use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_av_foundation::{
    AVError, AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem,
    AVPlayerItemDidPlayToEndTimeNotification, AVPlayerItemStatus, AVPlayerTimeControlStatus,
};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNotification,
    NSNotificationCenter, NSObjectNSKeyValueObserverRegistration, NSString, NSURL,
};

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlaybackState, PlayerEvent, PlayerOptions, VideoError, VideoSource};

/// The key path the item-readiness observer registers for (module doc's
/// observation table). Rooted at the **player**, not the item, so one
/// `addObserver:` target covers all three key paths and one teardown removes
/// them.
const KEY_PATH_ITEM_STATUS: &str = "currentItem.status";
/// The key path the play/pause/buffering observer registers for.
const KEY_PATH_TIME_CONTROL_STATUS: &str = "timeControlStatus";
/// The key path the decoded-geometry observer registers for.
const KEY_PATH_PRESENTATION_SIZE: &str = "currentItem.presentationSize";

/// `kCMTimeZero`, spelled as the literal it is defined to be rather than read
/// out of the framework's `extern` static: both seek tolerances, and the
/// restart position of a loop.
const CM_TIME_ZERO: CMTime = CMTime {
    value: 0,
    timescale: 1,
    flags: CMTimeFlags::Valid,
    epoch: 0,
};

/// The periodic time observer's interval — 0.25 s, expressed exactly as `1/4`
/// so no rounding is introduced by a seconds-to-`CMTime` conversion.
///
/// Four position events a second is the granularity a scrubber needs and no
/// more; AVFoundation may deliver fewer while stalled, and always delivers one
/// extra whenever time jumps or playback starts/stops.
const TIME_OBSERVER_INTERVAL: CMTime = CMTime {
    value: 1,
    timescale: 4,
    flags: CMTimeFlags::Valid,
    epoch: 0,
};

/// The timescale every seek position is expressed in — milliseconds, which is
/// the resolution [`crate::PlayerSnapshot::position`] itself carries, so the
/// conversion is exact rather than rounded.
const SEEK_TIMESCALE: i32 = 1000;

/// Cocoa's URL-loading error domain, matched by name.
///
/// Spelled as a literal rather than read from `NSURLErrorDomain`: that constant
/// lives behind `objc2-foundation`'s `NSURLError` feature, which this crate
/// does not enable, and the string is a frozen part of Foundation's public API.
const NS_URL_ERROR_DOMAIN: &str = "NSURLErrorDomain";

/// The next session id [`AppleBackend::open`] hands out.
///
/// Ids are minted here, not by a platform host (the Android backend's are
/// `openPlayer`'s return value): AVFoundation has no session table of its own,
/// so this module *is* the registry. Starts at `1`, so `0` is never a valid id
/// — the same "no session" reservation the other backends make.
static NEXT_SESSION_ID: AtomicI32 = AtomicI32::new(1);

/// Every live session, keyed by the id above (module doc's *Threading*).
///
/// A process-global registry is what lets [`player_for`] resolve a plain
/// integer — the only thing a platform-view params payload can carry — back to
/// the player, and what lets an observer holding nothing but a session id find
/// the [`Shared`] to publish into. Entries are inserted by
/// [`AppleBackend::open`] and removed by [`teardown`], which runs on the main
/// thread, so the Objective-C half of an entry is also *released* there.
///
/// `HashMap::new` is not `const`, so this is a [`LazyLock`] rather than a bare
/// `static`.
static SESSIONS: LazyLock<Mutex<HashMap<i32, Session>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether the process's iOS audio session has already been configured (module
/// doc's *iOS audio-session policy*) — set by the first open that reaches it,
/// and never cleared.
#[cfg(target_os = "ios")]
static AUDIO_SESSION_CONFIGURED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The session registry, poison-tolerant.
///
/// A panic caught by [`on_main`] while this lock was held would poison it and
/// turn every later video call into a panic of its own — exactly what must not
/// happen next to an FFI boundary (`docs/CODE_STANDARDS.md`). The map holds no
/// invariant a panic could half-break (it is a plain id → session table), so
/// recovering the guard is sound.
fn sessions() -> MutexGuard<'static, HashMap<i32, Session>> {
    SESSIONS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A value whose Objective-C contents are only ever constructed, messaged, and
/// released on the platform main thread.
///
/// `Retained<AVPlayer>` and friends are `!Send`/`!Sync` in `objc2`'s type
/// system (both classes are `MainThreadOnly`), but [`SESSIONS`] is a `static`,
/// which must be `Sync`. This wrapper is the one confined assertion that
/// bridges the two.
///
/// # Safety
///
/// Every `MainThreadBound` in this module is reachable only through
/// [`SESSIONS`], and every read of one goes through [`Self::get`] /
/// [`Self::get_mut`] / [`Self::into_inner`], each of which demands a
/// [`MainThreadMarker`] — proof the caller is on the main thread. The only
/// producers of that marker here are [`on_main`] (which runs its body on the
/// main thread by construction) and [`player_for`] (which answers `None`
/// without one). Removal from the map happens exclusively inside [`teardown`],
/// a main-thread body, so the contents are dropped there too and never on
/// another thread.
struct MainThreadBound<T> {
    value: T,
}

// SAFETY: see the type's `# Safety` doc — access is gated on a
// `MainThreadMarker`, so the contents never leave the main thread.
unsafe impl<T> Send for MainThreadBound<T> {}
// SAFETY: as above; a `&MainThreadBound<T>` yields a `&T` only against a
// `MainThreadMarker`.
unsafe impl<T> Sync for MainThreadBound<T> {}

impl<T> MainThreadBound<T> {
    fn new(value: T) -> Self {
        Self { value }
    }

    /// The wrapped value, against proof the caller is on the main thread.
    fn get(&self, _mtm: MainThreadMarker) -> &T {
        &self.value
    }

    /// The wrapped value mutably, against the same proof.
    fn get_mut(&mut self, _mtm: MainThreadMarker) -> &mut T {
        &mut self.value
    }

    /// Take the wrapped value, consuming the wrapper — [`teardown`]'s way of
    /// getting the Objective-C graph out of the map so it can be released
    /// without the registry lock held.
    fn into_inner(self, _mtm: MainThreadMarker) -> T {
        self.value
    }
}

/// One session's registry entry: the plain state any thread may read under the
/// lock, plus the Objective-C graph only the main thread may touch.
struct Session {
    /// Where this session publishes — cloned out before every publish, so the
    /// registry lock is never held across a listener call (module doc).
    shared: Arc<Shared>,
    /// Whether the item restarts at its end (module doc's *Looping*).
    looping: bool,
    /// The rate the next `play` applies. `1.0` unless
    /// [`AppleSession::set_rate`] stored another while the player was paused.
    rate: f32,
    /// The most recently requested volume, re-applied by [`construct`] so a
    /// [`AppleSession::set_volume`] issued before the player existed is not
    /// lost.
    volume: f32,
    /// Whether playback has been asked for — [`crate::PlayerOptions::autoplay`]
    /// at open, then every `play`/`pause`. [`construct`] honours it once the
    /// player exists, which is what makes a `play` issued during construction
    /// land rather than vanish.
    want_play: bool,
    /// Set by [`teardown`] before it removes the entry; every in-flight
    /// main-queue body checks it so a late observer cannot resurrect a closed
    /// session.
    closed: bool,
    /// The player graph, or `None` until [`construct`] has built it (module
    /// doc's *Threading*).
    objc: MainThreadBound<Option<AvObjects>>,
}

/// The AVFoundation object graph one open session owns — main-thread-confined
/// by the [`MainThreadBound`] it lives in.
struct AvObjects {
    /// The player itself, and what [`player_for`] hands a view factory.
    player: Retained<AVPlayer>,
    /// The item, held for the session's lifetime: the notification observer is
    /// scoped to it, and the status/duration/geometry reads go through it.
    item: Retained<AVPlayerItem>,
    /// The KVO + notification observer. Held strongly here, and holding nothing
    /// but a session id itself, so it cannot cycle with the player.
    observer: Retained<PlayerObserver>,
    /// The opaque token `addPeriodicTimeObserverForInterval:queue:usingBlock:`
    /// returned, which `removeTimeObserver:` takes back.
    ///
    /// `Option` because the token exists only *after* the observer is attached,
    /// and attaching happens after the graph is registered — the ordering the
    /// module doc's registry rule asks for (an observer must never be able to
    /// fire for a session the registry does not know yet).
    time_observer: Option<Retained<AnyObject>>,
}

/// A source resolved to something an `NSURL` can be built from, on the calling
/// thread, before any main-queue work is scheduled.
///
/// Resolution is what makes [`VideoError::UnsupportedSource`] a *synchronous*
/// answer from [`crate::VideoPlayer::open`] rather than a published error a
/// caller has to wait for.
enum ResolvedSource {
    /// An absolute filesystem path — [`VideoSource::File`], and a
    /// [`VideoSource::Asset`] once the main bundle has resolved it.
    File(String),
    /// A remote URL string AVFoundation will load ([`VideoSource::Url`]).
    ///
    /// Note that App Transport Security refuses plain `http` by default: an app
    /// serving `http` needs its own `NSAppTransportSecurity` exception, and
    /// without one the load fails later as a network error rather than being
    /// refused here.
    Url(String),
}

/// Opens AVFoundation player sessions, on iOS and macOS alike.
pub(crate) struct AppleBackend;

impl PlayerBackend for AppleBackend {
    type Session = AppleSession;

    /// Mint a session, hand its construction to the main queue, and return —
    /// without waiting for the item to load (the crate's threading contract).
    ///
    /// The source is resolved and validated **here**, on the calling thread, so
    /// a source AVFoundation could never accept is an immediate error rather
    /// than a published one. Everything after that — the `NSURL`, the item, the
    /// player, the observers — is built by [`construct`] on the main thread.
    ///
    /// # Errors
    /// [`VideoError::UnsupportedSource`] for a relative or non-UTF-8
    /// [`VideoSource::File`] path, a [`VideoSource::Asset`] the main bundle
    /// does not contain, or a [`VideoSource::Url`] Foundation cannot parse;
    /// [`VideoError::Host`] if the process has exhausted the session-id space.
    /// A failure to *load* is not reported here — it arrives as a published
    /// [`crate::PlayerEvent::Error`].
    fn open(
        source: &VideoSource,
        options: &PlayerOptions,
        shared: Arc<Shared>,
    ) -> Result<Self::Session, VideoError> {
        // Before anything else, and unconditionally: both hooks are idempotent
        // no-ops after the first call (module doc's *Factory registration*).
        #[cfg(target_os = "ios")]
        crate::ios_view::ensure_registered();
        #[cfg(target_os = "macos")]
        crate::macos_view::ensure_registered();

        let resolved = resolve_source(source)?;
        let id = next_session_id()?;

        // Registered before the construction hop, so an observer attached by
        // `construct` always finds its entry, and a control call issued while
        // the player is still being built records its intent instead of being
        // dropped (see `Session::want_play`).
        sessions().insert(
            id,
            Session {
                shared: Arc::clone(&shared),
                looping: options.looping,
                rate: 1.0,
                volume: options.volume,
                want_play: options.autoplay,
                closed: false,
                objc: MainThreadBound::new(None),
            },
        );

        // Published *before* the hop rather than after it: `on_main` runs
        // inline for a main-thread caller, so a construction that immediately
        // fails would otherwise have its `Error` overwritten by this `Loading`.
        // No listener can exist yet either way — `PlayerSession` is not built
        // until this call returns — so this publish reaches nobody, and the
        // snapshot a caller reads right after `open` is `Loading` exactly as
        // `VideoPlayer::open` documents.
        shared.publish(PlayerEvent::StateChanged(PlaybackState::Loading));

        let mix_with_others = options.mix_with_others;
        on_main(move |mtm| construct(id, &resolved, mix_with_others, mtm));

        Ok(AppleSession { id })
    }
}

/// One AVFoundation player session.
///
/// Holds only its id: everything else lives in [`SESSIONS`], because an
/// observer, a block, and [`player_for`] all reach the same state from a bare
/// integer and none of them has a handle to this struct. That is also what
/// makes the type trivially `Send + Sync`, as [`BackendSession`] requires.
pub(crate) struct AppleSession {
    /// The id the params payload carries and [`player_for`] resolves.
    id: i32,
}

impl BackendSession for AppleSession {
    /// Start (or resume) playback, applying the rate a previous
    /// [`Self::set_rate`] stored while paused.
    fn play(&self) -> Result<(), VideoError> {
        let id = self.id;
        on_main(move |mtm| {
            let Some(rate) = update_session(id, |session| {
                session.want_play = true;
                session.rate
            }) else {
                return;
            };
            let Some(live) = live(id, mtm) else {
                // Still constructing: `want_play` is recorded, and `construct`
                // starts playback as soon as the player exists.
                return;
            };
            start_playback(&live.player, rate);
        });
        Ok(())
    }

    fn pause(&self) -> Result<(), VideoError> {
        let id = self.id;
        on_main(move |mtm| {
            if update_session(id, |session| session.want_play = false).is_none() {
                return;
            }
            let Some(live) = live(id, mtm) else {
                return;
            };
            // SAFETY: a plain lifecycle message on a live player, on the main
            // thread (module doc's *Threading*).
            unsafe { live.player.pause() };
        });
        Ok(())
    }

    /// Seek with **zero tolerance** either side — sample-accurate, which costs
    /// extra decoding but is what a scrubber released on a frame expects — and
    /// publish one [`crate::PlayerEvent::Position`] when it lands.
    fn seek_to(&self, position: Duration) -> Result<(), VideoError> {
        let id = self.id;
        let time = cm_time_from(position);
        on_main(move |mtm| {
            let Some(live) = live(id, mtm) else {
                return;
            };
            let handler = RcBlock::new(move |_finished: Bool| {
                // A completion handler runs from an Objective-C frame:
                // `on_main` is what keeps a panic from unwinding into it. An
                // interrupted seek (`finished == NO`) still reports where the
                // player actually ended up, which is the useful answer.
                on_main(move |mtm| publish_current_position(id, mtm));
            });
            // SAFETY: `live.player` is a live player on the main thread, both
            // tolerances are `kCMTimeZero`, and `handler` is a live block of
            // the `void (^)(BOOL)` shape this method calls — AVFoundation
            // copies it, so its lifetime past this call is its own business.
            unsafe {
                live.player
                    .seekToTime_toleranceBefore_toleranceAfter_completionHandler(
                        time,
                        CM_TIME_ZERO,
                        CM_TIME_ZERO,
                        &handler,
                    );
            }
        });
        Ok(())
    }

    /// Set the playback rate, applying it now only if the player is actually
    /// moving; otherwise it is stored and applied by the next [`Self::play`].
    ///
    /// Setting a non-zero `rate` on a paused `AVPlayer` *starts* it, which is
    /// not what "set the rate" should mean on its own — so the platform's own
    /// answer to "are we moving?" (`timeControlStatus`) is what this reads,
    /// rather than a guess. [`crate::PlayerSession::set_rate`]'s documented
    /// "a non-zero rate on a paused player starts it" still holds through the
    /// stored value, one `play` later.
    fn set_rate(&self, rate: f32) -> Result<(), VideoError> {
        let id = self.id;
        on_main(move |mtm| {
            if update_session(id, |session| session.rate = rate).is_none() {
                return;
            }
            let Some(live) = live(id, mtm) else {
                return;
            };
            // SAFETY: a property read and write on a live player, on the main
            // thread.
            unsafe {
                if live.player.timeControlStatus() != AVPlayerTimeControlStatus::Paused {
                    live.player.setRate(rate);
                }
            }
        });
        Ok(())
    }

    fn set_volume(&self, volume: f32) -> Result<(), VideoError> {
        let id = self.id;
        on_main(move |mtm| {
            // Recorded as well as applied, so a volume set while the player is
            // still being built is not lost (`construct` re-applies it).
            if update_session(id, |session| session.volume = volume).is_none() {
                return;
            }
            let Some(live) = live(id, mtm) else {
                return;
            };
            // SAFETY: a property write on a live player, on the main thread.
            // AVFoundation clamps the value itself.
            unsafe { live.player.setVolume(volume) };
        });
        Ok(())
    }

    fn set_looping(&self, looping: bool) -> Result<(), VideoError> {
        let id = self.id;
        on_main(move |mtm| {
            if update_session(id, |session| session.looping = looping).is_none() {
                return;
            }
            let Some(live) = live(id, mtm) else {
                return;
            };
            // SAFETY: a property write on a live player, on the main thread.
            unsafe { live.player.setActionAtItemEnd(action_at_item_end(looping)) };
        });
        Ok(())
    }

    /// Release the player — on the main queue, without ever blocking the
    /// caller, and idempotently.
    ///
    /// [`crate::PlayerSession::close`]'s own flag already makes the *public*
    /// call once-only, but this is idempotent regardless: a listener may close
    /// its own session from inside an event, and a second teardown for an id
    /// whose entry is gone is a no-op rather than an unbalanced
    /// `removeObserver:` (which Objective-C answers with an exception).
    fn close(&self) {
        let id = self.id;
        on_main(move |mtm| teardown(id, mtm));
    }

    fn id(&self) -> u32 {
        // Ids are minted positive by `next_session_id`, which refuses to hand
        // out anything else, so this conversion never loses one.
        u32::try_from(self.id).unwrap_or(0)
    }
}

/// Look up the live player behind a session id — how the Rust view factories on
/// both Apple platforms attach an `AVPlayerLayer` to a session without any FFI
/// export (module doc's *accessor contract*).
///
/// The id is signed because it arrives from the native side as the params
/// payload's session number, and a stale or hostile payload may carry anything;
/// an unknown id answers `None` rather than failing. `None` also covers a
/// closed session, and one whose player is still being constructed — a factory
/// that gets `None` should attach nothing and wait for the next update rather
/// than treat it as an error.
///
/// Callable only on the main thread: an `AVPlayerLayer` could not be attached
/// from anywhere else, so an off-thread call is a caller bug — logged and
/// answered `None` rather than asserted.
// No caller until the view factories exist (v3-02/v3-03); this is the frozen
// seam they are written against.
#[allow(dead_code)]
pub(crate) fn player_for(session: i32) -> Option<Retained<AVPlayer>> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!(
            "frust-video-player: player_for({session}) called off the main thread — answering None"
        );
        return None;
    };
    let sessions = sessions();
    let entry = sessions.get(&session)?;
    if entry.closed {
        return None;
    }
    entry
        .objc
        .get(mtm)
        .as_ref()
        .map(|objects| objects.player.clone())
}

// --- The main-thread hop ------------------------------------------------------

/// Run `body` on the platform main thread: inline when the caller is already
/// there, `dispatch_async` onto the main queue when it is not.
///
/// Asynchronous, never `exec_sync`: this is the hop every control method and
/// every teardown takes, and parking a caller on the main queue would be either
/// pointless (it *is* the main queue) or, from a worker, a blocking call the
/// crate promises never to make.
///
/// Also the module's single no-unwind boundary: a body that panics is caught and
/// logged here, so nothing ever unwinds into the Objective-C runtime or out of a
/// `libdispatch` callback (`docs/CODE_STANDARDS.md`'s Plugin Conventions — this
/// crate cannot use `frust-shell-common`'s `guard`, which lives above the plugin
/// charter line).
fn on_main(body: impl FnOnce(MainThreadMarker) + Send + 'static) {
    let run = move || {
        let Some(mtm) = MainThreadMarker::new() else {
            // Only reachable if libdispatch ran a main-queue block off the main
            // thread, which it does not; logged rather than asserted so an
            // impossible case is still diagnosable.
            log::error!("frust-video-player: main-queue body ran off the main thread — skipped");
            return;
        };
        if catch_unwind(AssertUnwindSafe(|| body(mtm))).is_err() {
            log::error!("frust-video-player: panic caught on the AVFoundation main-thread path");
        }
    };

    if MainThreadMarker::new().is_some() {
        run();
    } else {
        DispatchQueue::main().exec_async(run);
    }
}

// --- Registry access (never holding the lock across a player call or publish) -

/// The pieces a main-thread body needs, lifted out of the registry so no player
/// message is ever sent — and no event ever published — with the lock held
/// (module doc). Lifting them costs one `retain` each, which is the only
/// Objective-C traffic that rule admits.
struct Live {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    shared: Arc<Shared>,
}

/// The live graph plus snapshot handle for `session`, or `None` when the
/// session is unknown, closed, or still being constructed.
fn live(session: i32, mtm: MainThreadMarker) -> Option<Live> {
    let sessions = sessions();
    let entry = sessions.get(&session)?;
    if entry.closed {
        return None;
    }
    let objects = entry.objc.get(mtm).as_ref()?;
    Some(Live {
        player: objects.player.clone(),
        item: objects.item.clone(),
        shared: Arc::clone(&entry.shared),
    })
}

/// The [`Shared`] a callback should publish into, cloned **out** of the registry
/// so the lock is released before the listener runs. `None` — an answer for a
/// session this side no longer knows — is the caller's cue to drop the event.
fn shared_of(session: i32) -> Option<Arc<Shared>> {
    let sessions = sessions();
    let entry = sessions.get(&session)?;
    if entry.closed {
        return None;
    }
    Some(Arc::clone(&entry.shared))
}

/// Mutate a live session's plain state under the lock and return whatever
/// `update` produced, or `None` for an unknown or closed session.
///
/// `update` touches only the fields any thread may read; the Objective-C half
/// needs a [`MainThreadMarker`] and is reached through [`live`] instead.
fn update_session<R>(session: i32, update: impl FnOnce(&mut Session) -> R) -> Option<R> {
    let mut sessions = sessions();
    let entry = sessions.get_mut(&session)?;
    if entry.closed {
        return None;
    }
    Some(update(entry))
}

/// The next session id, or a typed refusal if the id space is exhausted.
///
/// # Errors
/// [`VideoError::Host`] after `i32::MAX` opens in one process — unreachable in
/// practice, and a typed error rather than a wrapped negative id
/// [`AppleSession::id`] could not represent.
fn next_session_id() -> Result<i32, VideoError> {
    let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    if id > 0 {
        return Ok(id);
    }
    Err(VideoError::Host(
        "apple video backend: the session id space is exhausted".to_string(),
    ))
}

// --- Construction --------------------------------------------------------------

/// Build the player graph for `session` and wire its observation up — the whole
/// main-thread half of [`AppleBackend::open`].
///
/// Ordering is deliberate and is the module doc's registry rule in code: the
/// graph is registered **before** any observer is attached, so no observer can
/// fire for a session the registry cannot resolve.
fn construct(session: i32, source: &ResolvedSource, mix_with_others: bool, mtm: MainThreadMarker) {
    #[cfg(target_os = "ios")]
    configure_audio_session_once(mix_with_others);
    // macOS has no `AVAudioSession`; the option is documented as iOS-only on
    // `PlayerOptions::mix_with_others` and is deliberately not emulated here.
    #[cfg(not(target_os = "ios"))]
    let _ = mix_with_others;

    let Some(shared) = shared_of(session) else {
        // Closed between `open` returning and this body running.
        return;
    };

    let Some(url) = build_url(source) else {
        let error = VideoError::UnsupportedSource(format!(
            "apple video backend: could not build an NSURL for `{}`",
            source.describe()
        ));
        shared.publish(PlayerEvent::Error(error));
        shared.publish(PlayerEvent::StateChanged(PlaybackState::Error));
        return;
    };

    // SAFETY: `url` is a live `NSURL`, and both constructors are the documented
    // main-thread factory methods for their (`MainThreadOnly`) classes — the
    // marker is the proof they demand.
    let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };

    let Some((volume, looping, want_play, rate)) = update_session(session, |entry| {
        (entry.volume, entry.looping, entry.want_play, entry.rate)
    }) else {
        return;
    };

    // SAFETY: property writes on the player just constructed above, on the main
    // thread. AVFoundation clamps the volume itself.
    unsafe {
        player.setVolume(volume);
        player.setActionAtItemEnd(action_at_item_end(looping));
    }

    let observer = PlayerObserver::new(session);

    // Registration, before a single observer is attached.
    let registered = update_session(session, |entry| {
        *entry.objc.get_mut(mtm) = Some(AvObjects {
            player: player.clone(),
            item: item.clone(),
            observer: observer.clone(),
            time_observer: None,
        });
    });
    if registered.is_none() {
        // Closed while the graph was being built: nothing is attached yet, so
        // dropping the locals here is the whole teardown.
        return;
    }

    attach_observers(session, &player, &item, &observer, mtm);

    if want_play {
        start_playback(&player, rate);
    }
}

/// Attach the three KVO key paths, the end-of-item notification, and the
/// periodic time observer (module doc's observation table).
fn attach_observers(
    session: i32,
    player: &AVPlayer,
    item: &AVPlayerItem,
    observer: &PlayerObserver,
    mtm: MainThreadMarker,
) {
    let target: &NSObject = observer;

    // `New` rather than `New | Initial`: an initial callback would publish the
    // item's not-yet-known state straight back over the `Loading` that `open`
    // just published, and every value observed here reports a change of its own
    // the moment it has one.
    let options = NSKeyValueObservingOptions::New;
    for key_path in [
        KEY_PATH_ITEM_STATUS,
        KEY_PATH_TIME_CONTROL_STATUS,
        KEY_PATH_PRESENTATION_SIZE,
    ] {
        let key_path = NSString::from_str(key_path);
        // SAFETY: `target` is a live `PlayerObserver`, which implements
        // `observeValueForKeyPath:ofObject:change:context:`; the key paths are
        // the module doc's three, all valid on `AVPlayer`; and the context is
        // null, which the observer never reads (it dispatches on the key path).
        // Each is balanced by exactly one `removeObserver:forKeyPath:` in
        // `detach`.
        unsafe {
            player.addObserver_forKeyPath_options_context(
                target,
                &key_path,
                options,
                ptr::null_mut(),
            );
        }
    }

    // SAFETY: reading an `extern` AVFoundation constant static (edition-2024
    // unsafe); linker-provided and non-null wherever AVFoundation is linked.
    let name = unsafe { AVPlayerItemDidPlayToEndTimeNotification };
    let item_object: &AnyObject = item;
    // SAFETY: `target` implements the selector named below, `name` is the
    // framework's own notification name, and scoping the registration to this
    // session's own item is what keeps one session's end from waking another's
    // observer. Balanced by `removeObserver:name:object:` in `detach`.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            target,
            sel!(frustVideoPlayerItemDidPlayToEnd:),
            Some(name),
            Some(item_object),
        );
    }

    let block = RcBlock::new(move |time: CMTime| {
        // Enqueued on the main queue below, so this already *is* the main
        // thread; `on_main` runs it inline and supplies the no-unwind boundary
        // an Objective-C block needs.
        on_main(move |mtm| publish_position(session, time, mtm));
    });
    // SAFETY: `player` is live and on the main thread, the queue is the main
    // queue (serial, as this method requires), and `block` is a live block of
    // the `void (^)(CMTime)` shape it calls. The returned token is retained
    // below for as long as the observer must fire, and handed back to
    // `removeTimeObserver:` in `detach` — the pairing this method's own doc
    // demands.
    let token = unsafe {
        player.addPeriodicTimeObserverForInterval_queue_usingBlock(
            TIME_OBSERVER_INTERVAL,
            Some(DispatchQueue::main()),
            &block,
        )
    };
    let stored = update_session(session, |entry| {
        if let Some(objects) = entry.objc.get_mut(mtm).as_mut() {
            objects.time_observer = Some(token.clone());
        }
    });
    if stored.is_none() {
        // Closed in the window between registration and here: the token would
        // otherwise never reach `detach`, so remove it now.
        // SAFETY: `token` is exactly what this player's
        // `addPeriodicTimeObserverForInterval:` returned moments ago — the one
        // argument `removeTimeObserver:` accepts without throwing.
        unsafe { player.removeTimeObserver(&token) };
    }
}

/// `play()`, or `setRate:` when a rate other than normal speed was asked for —
/// the one place playback is started, so the stored-rate rule lives in exactly
/// one function.
fn start_playback(player: &AVPlayer, rate: f32) {
    // SAFETY: lifecycle messages on a live player, on the main thread.
    unsafe {
        if (rate - 1.0).abs() > f32::EPSILON {
            player.setRate(rate);
        } else {
            player.play();
        }
    }
}

/// What the player does when the item ends: nothing while looping (the
/// end-of-item observer restarts it), otherwise pause (module doc's *Looping*).
fn action_at_item_end(looping: bool) -> AVPlayerActionAtItemEnd {
    if looping {
        AVPlayerActionAtItemEnd::None
    } else {
        AVPlayerActionAtItemEnd::Pause
    }
}

// --- Teardown ------------------------------------------------------------------

/// Release `session`'s player graph and drop its registry entry — idempotent,
/// and always on the main thread.
///
/// The entry is *removed* under the lock and everything else happens after it is
/// released: unregistering an observer, pausing, and clearing the item are all
/// Objective-C, and the final `StateChanged(Idle)` runs an app listener.
fn teardown(session: i32, mtm: MainThreadMarker) {
    let entry = {
        let mut sessions = sessions();
        match sessions.get_mut(&session) {
            // Already closed (or closing): nothing left to do, and re-running
            // the removals below would be an unbalanced `removeObserver:`.
            Some(entry) if entry.closed => return,
            Some(entry) => entry.closed = true,
            None => return,
        }
        sessions.remove(&session)
    };
    let Some(entry) = entry else {
        return;
    };

    let shared = Arc::clone(&entry.shared);
    if let Some(objects) = entry.objc.into_inner(mtm) {
        detach(&objects);
    }
    shared.publish(PlayerEvent::StateChanged(PlaybackState::Idle));
}

/// Unwire one session's graph, in the only order that is safe: stop the
/// callbacks first, *then* stop the player, *then* let go of the item.
///
/// Removing the observers before `replaceCurrentItemWithPlayerItem:` matters —
/// clearing the item is itself an observable change on two of the three key
/// paths, and a callback arriving mid-teardown would be looking up a session
/// that has already left the registry.
fn detach(objects: &AvObjects) {
    if let Some(token) = objects.time_observer.as_ref() {
        // SAFETY: `token` is what this same player's
        // `addPeriodicTimeObserverForInterval:` returned, which is the one
        // argument `removeTimeObserver:` accepts without throwing.
        unsafe { objects.player.removeTimeObserver(token) };
    }

    let target: &NSObject = &objects.observer;
    for key_path in [
        KEY_PATH_ITEM_STATUS,
        KEY_PATH_TIME_CONTROL_STATUS,
        KEY_PATH_PRESENTATION_SIZE,
    ] {
        let key_path = NSString::from_str(key_path);
        // SAFETY: exactly balances the `addObserver:forKeyPath:options:context:`
        // in `attach_observers` — same observer, same key path, same player —
        // which is what keeps KVO from raising on an unregistered observer.
        unsafe { objects.player.removeObserver_forKeyPath(target, &key_path) };
    }

    // SAFETY: reading an `extern` AVFoundation constant static (edition-2024
    // unsafe), then removing exactly the registration `attach_observers` made.
    // Removing an observer that was never registered is a documented no-op, so
    // this is safe even if the notification path was never reached.
    unsafe {
        let name = AVPlayerItemDidPlayToEndTimeNotification;
        let item_object: &AnyObject = &objects.item;
        NSNotificationCenter::defaultCenter().removeObserver_name_object(
            target,
            Some(name),
            Some(item_object),
        );
    }

    // SAFETY: lifecycle messages on a live player, on the main thread, with
    // every callback already unwired above. Clearing the item is what actually
    // releases the decoder and the network connection behind it.
    unsafe {
        objects.player.pause();
        objects.player.replaceCurrentItemWithPlayerItem(None);
    }
}

// --- Observation handlers -------------------------------------------------------

/// `currentItem.status` changed — the readiness edge (module doc's table).
fn handle_item_status(session: i32, mtm: MainThreadMarker) {
    let Some(live) = live(session, mtm) else {
        return;
    };
    // SAFETY: a property read on a live item, on the main thread.
    let status = unsafe { live.item.status() };

    if status == AVPlayerItemStatus::ReadyToPlay {
        // The duration is knowable from here on, so the position event that
        // carries it is published before the state that says "ready".
        live.shared
            .publish(position_event(&live.player, &live.item));
        // SAFETY: a property read on a live player, on the main thread.
        let control = unsafe { live.player.timeControlStatus() };
        live.shared
            .publish(PlayerEvent::StateChanged(state_for(control)));
        return;
    }

    if status == AVPlayerItemStatus::Failed {
        // SAFETY: as above; `error` is non-nil exactly in the failed status.
        let error = unsafe { live.item.error() };
        let error = error.as_deref().map_or_else(
            || {
                VideoError::Decoder(
                    "apple video backend: the item failed with no reported error".to_string(),
                )
            },
            video_error_from,
        );
        // `Error` already moves the snapshot to `PlaybackState::Error`; the
        // explicit `StateChanged` after it is for a listener that watches only
        // state transitions.
        live.shared.publish(PlayerEvent::Error(error));
        live.shared
            .publish(PlayerEvent::StateChanged(PlaybackState::Error));
        return;
    }

    // `Unknown` — still preparing. Anything else is a status newer than this
    // crate, and reads the same way rather than as an error.
    live.shared
        .publish(PlayerEvent::StateChanged(PlaybackState::Loading));
}

/// `timeControlStatus` changed — the play/pause/buffering edge.
fn handle_time_control_status(session: i32, mtm: MainThreadMarker) {
    let Some(live) = live(session, mtm) else {
        return;
    };
    // SAFETY: a property read on a live player, on the main thread.
    let state = state_for(unsafe { live.player.timeControlStatus() });

    // A finished item drops to `paused`, and a failed one may too, but neither
    // is news: reporting it would overwrite `Ended`/`Error` with a state that
    // says the session is fine. Both are terminal until something else moves
    // the player, and a real resume arrives as `Playing`, which is published.
    if state == PlaybackState::Paused
        && matches!(
            live.shared.snapshot().state,
            PlaybackState::Ended | PlaybackState::Error
        )
    {
        return;
    }

    live.shared.publish(PlayerEvent::StateChanged(state));
}

/// `currentItem.presentationSize` changed — the decoded geometry.
fn handle_presentation_size(session: i32, mtm: MainThreadMarker) {
    let Some(live) = live(session, mtm) else {
        return;
    };
    // SAFETY: a property read on a live item, on the main thread.
    let size = unsafe { live.item.presentationSize() };
    if let Some(event) = video_size_event(size.width, size.height) {
        live.shared.publish(event);
    }
}

/// `AVPlayerItemDidPlayToEndTime` — restart while looping, otherwise report
/// [`PlaybackState::Ended`] (module doc's *Looping*).
fn handle_did_play_to_end(session: i32, mtm: MainThreadMarker) {
    let Some(looping) = update_session(session, |entry| entry.looping) else {
        return;
    };
    let Some(live) = live(session, mtm) else {
        return;
    };

    if !looping {
        live.shared
            .publish(PlayerEvent::StateChanged(PlaybackState::Ended));
        return;
    }

    // SAFETY: a seek on a live player, on the main thread. `actionAtItemEnd` is
    // `.none` while looping, so the player is sitting at the end rather than
    // paused, and the rate it resumes at is whatever `start_playback` applies.
    unsafe { live.player.seekToTime(CM_TIME_ZERO) };
    let rate = update_session(session, |entry| entry.rate).unwrap_or(1.0);
    start_playback(&live.player, rate);
}

/// Publish one [`PlayerEvent::Position`] carrying the player's current time —
/// the seek completion handler's whole job.
fn publish_current_position(session: i32, mtm: MainThreadMarker) {
    let Some(live) = live(session, mtm) else {
        return;
    };
    live.shared
        .publish(position_event(&live.player, &live.item));
}

/// Publish one [`PlayerEvent::Position`] at `time` — the periodic observer's
/// whole job, using the time the block was handed rather than re-reading it.
fn publish_position(session: i32, time: CMTime, mtm: MainThreadMarker) {
    let Some(live) = live(session, mtm) else {
        return;
    };
    live.shared.publish(PlayerEvent::Position {
        position: duration_from(time).unwrap_or_default(),
        duration: item_duration(&live.item),
    });
}

/// A [`PlayerEvent::Position`] read straight off the player and its item.
fn position_event(player: &AVPlayer, item: &AVPlayerItem) -> PlayerEvent {
    // SAFETY: a property read on a live player, on the main thread.
    let position = unsafe { player.currentTime() };
    PlayerEvent::Position {
        position: duration_from(position).unwrap_or_default(),
        duration: item_duration(item),
    }
}

/// The item's duration, or `None` while it is not a finite number — which is
/// what `kCMTimeIndefinite` means before the item is ready, and what a live
/// stream reports forever.
fn item_duration(item: &AVPlayerItem) -> Option<Duration> {
    // SAFETY: a property read on a live item, on the main thread.
    duration_from(unsafe { item.duration() })
}

// --- The observer class -----------------------------------------------------------

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The ivars are a plain session id (`i32`) with no `Drop` impl, so the
    //   macro's generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // `AVPlayer` serializes its KVO notifications onto the main queue by default
    // and the end-of-item notification is posted by AVFoundation's own
    // machinery, but neither is a guarantee this class can rely on for
    // construction, and every handler hops through `on_main` regardless — so the
    // class itself is usable from any thread.
    #[thread_kind = AnyThread]
    #[ivars = i32]
    struct PlayerObserver;

    unsafe impl NSObjectProtocol for PlayerObserver {}

    impl PlayerObserver {
        /// The KVO callback for all three key paths this module registers.
        ///
        /// Deliberately holds nothing but the session id: a strong reference to
        /// the player here would cycle with the player's own strong reference
        /// to this observer (held by [`AvObjects`]), and neither would ever be
        /// released.
        ///
        /// The change dictionary is ignored — every handler re-reads the live
        /// property instead, because the useful answer is never one value
        /// (readiness needs the duration and the time-control status too), and
        /// re-reading cannot disagree with the object's actual state the way a
        /// queued change notification can.
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            let session = *self.ivars();
            let Some(key_path) = key_path.map(NSString::to_string) else {
                log::debug!(
                    "frust-video-player: KVO callback for session {session} carried no key path"
                );
                return;
            };
            on_main(move |mtm| match key_path.as_str() {
                KEY_PATH_ITEM_STATUS => handle_item_status(session, mtm),
                KEY_PATH_TIME_CONTROL_STATUS => handle_time_control_status(session, mtm),
                KEY_PATH_PRESENTATION_SIZE => handle_presentation_size(session, mtm),
                other => log::debug!(
                    "frust-video-player: unexpected KVO key path `{other}` for session {session}"
                ),
            });
        }

        /// `AVPlayerItemDidPlayToEndTimeNotification` for this session's own
        /// item — the registration is scoped to it, so no other session's end
        /// reaches here.
        #[unsafe(method(frustVideoPlayerItemDidPlayToEnd:))]
        fn did_play_to_end(&self, _notification: &NSNotification) {
            let session = *self.ivars();
            on_main(move |mtm| handle_did_play_to_end(session, mtm));
        }
    }
);

impl PlayerObserver {
    /// An observer for one session, not yet attached to anything —
    /// [`attach_observers`] is the whole wiring surface.
    fn new(session: i32) -> Retained<Self> {
        let this = Self::alloc().set_ivars(session);
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set (the
        // `plugins/camera/src/apple.rs` `PhotoCaptureDelegate::new` idiom).
        unsafe { msg_send![super(this), init] }
    }
}

// --- iOS audio session -------------------------------------------------------------

/// Configure the process's shared `AVAudioSession` for playback, once (module
/// doc's *iOS audio-session policy*).
///
/// Every failure is logged and swallowed: an app whose audio session another
/// component already owns still gets a picture, and refusing to open a video
/// because a category could not be set would be the wrong trade.
#[cfg(target_os = "ios")]
fn configure_audio_session_once(mix_with_others: bool) {
    use std::sync::atomic::Ordering::{AcqRel, Acquire};

    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayback,
    };

    if AUDIO_SESSION_CONFIGURED
        .compare_exchange(false, true, AcqRel, Acquire)
        .is_err()
    {
        return;
    }

    // SAFETY: reading an `extern` AVFAudio constant static (edition-2024
    // unsafe); the binding models it as optional, and a missing category is
    // reported rather than assumed away.
    let Some(category) = (unsafe { AVAudioSessionCategoryPlayback }) else {
        log::warn!(
            "frust-video-player: AVAudioSessionCategoryPlayback is unavailable — leaving the \
             audio session alone"
        );
        return;
    };

    let mut options = AVAudioSessionCategoryOptions::empty();
    if mix_with_others {
        options |= AVAudioSessionCategoryOptions::MixWithOthers;
    }

    // SAFETY: `sharedInstance` is the framework's own singleton accessor, and
    // both setters take the category/options pair validated above. The class is
    // documented thread-safe (its binding is `Send + Sync`), so this needs no
    // main-thread hop of its own.
    unsafe {
        let audio = AVAudioSession::sharedInstance();
        if let Err(error) = audio.setCategory_withOptions_error(category, options) {
            log::warn!(
                "frust-video-player: could not set the playback audio category ({}) — audio may \
                 be silenced by the ringer switch",
                describe(&error)
            );
        }
        if let Err(error) = audio.setActive_error(true) {
            log::warn!(
                "frust-video-player: could not activate the audio session ({})",
                describe(&error)
            );
        }
    }
}

// --- Source resolution ---------------------------------------------------------------

impl ResolvedSource {
    /// This source as it should read in an error message.
    fn describe(&self) -> &str {
        match self {
            Self::File(path) => path,
            Self::Url(url) => url,
        }
    }
}

/// Resolve a [`VideoSource`] to something an `NSURL` can be built from, on the
/// calling thread.
///
/// # Errors
/// [`VideoError::UnsupportedSource`] for a relative or non-UTF-8 file path
/// (matching the Android backend, which refuses both for the same reason — the
/// platform resolves nothing, so a relative path would be read against whatever
/// working directory the process happens to have), an asset the main bundle does
/// not contain, or a URL Foundation will not parse.
fn resolve_source(source: &VideoSource) -> Result<ResolvedSource, VideoError> {
    match source {
        VideoSource::File(path) => {
            if !path.is_absolute() {
                return Err(VideoError::UnsupportedSource(format!(
                    "a file source must be an absolute path, got `{}`",
                    path.display()
                )));
            }
            path.to_str()
                .map(|path| ResolvedSource::File(path.to_owned()))
                .ok_or_else(|| {
                    VideoError::UnsupportedSource(format!(
                        "a file source must be valid UTF-8, got `{}`",
                        path.display()
                    ))
                })
        }
        VideoSource::Asset(name) => {
            let path = bundle_resource_path(name).ok_or_else(|| {
                VideoError::UnsupportedSource(format!(
                    "the main bundle contains no resource `{name}`"
                ))
            })?;
            Ok(ResolvedSource::File(path))
        }
        VideoSource::Url(url) => {
            // Parsed here so an unusable URL is a synchronous refusal; the
            // `NSURL` the item is actually built from is created on the main
            // thread by `construct` (a `Retained<NSURL>` is not `Send`).
            if NSURL::URLWithString(&NSString::from_str(url)).is_none() {
                return Err(VideoError::UnsupportedSource(format!(
                    "`{url}` is not a URL Foundation can parse"
                )));
            }
            Ok(ResolvedSource::Url(url.clone()))
        }
    }
}

/// `[[NSBundle mainBundle] resourcePath]` joined with `name`, if the file is
/// actually there.
///
/// `NSBundle` is reached through a runtime class lookup rather than the
/// `objc2-foundation` binding on purpose: the binding sits behind that crate's
/// `NSBundle` feature, which this crate's manifest does not enable, and widening
/// a shared feature list for one call is a worse trade than four lines of
/// `msg_send!`. The class is part of Foundation itself, so the lookup fails only
/// in a process with no Foundation loaded — where nothing else in this module
/// would work either.
///
/// Existence is checked with the standard library rather than
/// `pathForResource:ofType:`, because a bundled asset is named by *path*
/// (`clips/intro.mp4`), which that API's name/extension split does not model.
fn bundle_resource_path(name: &str) -> Option<String> {
    let class = AnyClass::get(c"NSBundle")?;
    // SAFETY: `+[NSBundle mainBundle]` takes no arguments and returns the
    // process's main bundle (nil only in a process without one), and
    // `-resourcePath` returns its resource directory or nil. Both are
    // autoreleased returns, which `Retained` handles.
    let bundle: Option<Retained<AnyObject>> = unsafe { msg_send![class, mainBundle] };
    let bundle = bundle?;
    // SAFETY: as above.
    let resources: Option<Retained<NSString>> = unsafe { msg_send![&*bundle, resourcePath] };
    let resources = resources?.to_string();

    let path = Path::new(&resources).join(name);
    if !path.is_file() {
        return None;
    }
    path.to_str().map(str::to_owned)
}

/// The `NSURL` an item is built from — a file URL for a path, a parsed one for a
/// remote source.
fn build_url(source: &ResolvedSource) -> Option<Retained<NSURL>> {
    match source {
        ResolvedSource::File(path) => Some(NSURL::fileURLWithPath(&NSString::from_str(path))),
        // Already parsed once by `resolve_source`; `None` here would mean the
        // string changed underneath us, which it cannot.
        ResolvedSource::Url(url) => NSURL::URLWithString(&NSString::from_str(url)),
    }
}

// --- Pure helpers (no Objective-C: unit-tested below) ----------------------------------

/// A [`PlaybackState`] for one `timeControlStatus`.
///
/// A status outside the framework's three reads as `Paused`: a player that is
/// not known to be moving is the safe report, and a newer OS adding a fourth
/// must not turn into a panic here.
fn state_for(status: AVPlayerTimeControlStatus) -> PlaybackState {
    if status == AVPlayerTimeControlStatus::Playing {
        PlaybackState::Playing
    } else if status == AVPlayerTimeControlStatus::WaitingToPlayAtSpecifiedRate {
        PlaybackState::Buffering
    } else {
        PlaybackState::Paused
    }
}

/// A seek position as the `CMTime` AVFoundation takes, in milliseconds —
/// saturating rather than wrapping, so a nonsense position never turns into a
/// small one.
fn cm_time_from(position: Duration) -> CMTime {
    let millis = i64::try_from(position.as_millis()).unwrap_or(i64::MAX);
    CMTime {
        value: millis,
        timescale: SEEK_TIMESCALE,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

/// A `CMTime` as a [`Duration`], or `None` when it carries no finite number.
///
/// `kCMTimeIndefinite` (an unready item, and a live stream forever),
/// `kCMTimeInvalid`, and both infinities all answer `None`, which is exactly how
/// [`crate::PlayerSnapshot::duration`] spells "not known yet". A negative time
/// answers `None` too rather than saturating to zero: a negative duration is not
/// a duration.
fn duration_from(time: CMTime) -> Option<Duration> {
    if !time.flags.contains(CMTimeFlags::Valid) {
        return None;
    }
    if time.flags.intersects(CMTimeFlags::ImpliedValueFlagsMask) {
        return None;
    }
    let timescale = time.timescale;
    if timescale <= 0 {
        return None;
    }
    let value = time.value;
    if value < 0 {
        return None;
    }
    // `value / timescale` seconds, taken in milliseconds so the whole
    // computation stays in integers: no float rounding, and no overflow that
    // `saturating_mul` does not already answer.
    let millis = value.saturating_mul(1000) / i64::from(timescale);
    u64::try_from(millis).ok().map(Duration::from_millis)
}

/// Build the [`PlayerEvent::VideoSize`] a `presentationSize` reports, or `None`
/// for a non-positive pair.
///
/// [`crate::PlayerSnapshot::video_size`] spells "not known yet" as `None`, and
/// `presentationSize` is `CGSizeZero` until the geometry is known (and stays
/// there for an audio-only item), so a zero is dropped rather than published as
/// a size no slot could be laid out from.
fn video_size_event(width: f64, height: f64) -> Option<PlayerEvent> {
    if !(width.is_finite() && height.is_finite()) || width < 1.0 || height < 1.0 {
        return None;
    }
    Some(PlayerEvent::VideoSize {
        width: width.round() as u32,
        height: height.round() as u32,
    })
}

/// Map an `NSError` AVFoundation reported onto the [`VideoError`] variant a
/// caller matches on (the crate doc's retry-a-`Network`, report-a-`Decoder`
/// distinction).
///
/// Anything Cocoa's URL loading system raised is a transport failure by
/// definition; the AVFoundation codes are split between "this media will never
/// play here" ([`VideoError::UnsupportedSource`]) and "the transport gave up"
/// ([`VideoError::Network`]), with everything else a [`VideoError::Decoder`] —
/// the variant whose own doc covers an unsupported codec *or* corrupt data.
fn video_error_from(error: &NSError) -> VideoError {
    if error.domain().to_string() == NS_URL_ERROR_DOMAIN {
        return VideoError::Network(describe(error));
    }

    let code = error.code();
    if [
        AVError::NoLongerPlayable,
        AVError::ServerIncorrectlyConfigured,
        AVError::FailedToLoadMediaData,
        AVError::ContentIsUnavailable,
    ]
    .iter()
    .any(|known| known.0 == code)
    {
        return VideoError::Network(describe(error));
    }

    if [
        AVError::FileFormatNotRecognized,
        AVError::FormatUnsupported,
        AVError::UndecodableMediaData,
        AVError::DecoderNotFound,
        AVError::InvalidSourceMedia,
        AVError::NoSourceTrack,
    ]
    .iter()
    .any(|known| known.0 == code)
    {
        return VideoError::UnsupportedSource(describe(error));
    }

    VideoError::Decoder(describe(error))
}

/// An `NSError` rendered for a [`VideoError`] message (the
/// `plugins/camera/src/apple.rs` shape).
fn describe(error: &NSError) -> String {
    format!(
        "apple video backend: {}, domain {}, code {}",
        error.localizedDescription(),
        error.domain(),
        error.code()
    )
}

#[cfg(test)]
mod tests {
    //! These are **compile-time** checks as much as behavioral ones: the whole
    //! module compiles only on an Apple target, so `cargo check/clippy --target
    //! aarch64-apple-ios-sim --all-targets` is what exercises them from a
    //! non-Apple host, and *running* them needs an Apple one. Only the pure
    //! helpers are testable at all — everything else needs a live `AVPlayer`,
    //! which is the device gate's job.

    use std::time::Duration;

    use objc2_av_foundation::{AVPlayerActionAtItemEnd, AVPlayerTimeControlStatus};
    use objc2_core_media::{CMTime, CMTimeFlags};

    use crate::{PlaybackState, PlayerEvent};

    /// A seek position becomes a millisecond-scaled `CMTime`.
    #[test]
    fn seek_positions_convert_to_millisecond_cm_times() {
        let time = super::cm_time_from(Duration::from_millis(2_500));
        // Copied out field by field: `CMTime` is `repr(C, packed(4))`, so
        // `assert_eq!`'s reference to a field would be an unaligned one.
        let (value, timescale, flags) = (time.value, time.timescale, time.flags);
        assert_eq!(value, 2_500);
        assert_eq!(timescale, super::SEEK_TIMESCALE);
        assert!(flags.contains(CMTimeFlags::Valid));
    }

    /// A numeric `CMTime` round-trips back to the duration it names.
    #[test]
    fn numeric_cm_times_become_durations() {
        let time = CMTime {
            value: 90_000,
            timescale: 600,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        assert_eq!(
            super::duration_from(time),
            Some(Duration::from_millis(150_000))
        );
    }

    /// Indefinite, invalid, and negative times are "not known yet", never a zero
    /// duration — the distinction `PlayerSnapshot::duration` is built on.
    #[test]
    fn non_numeric_cm_times_have_no_duration() {
        let indefinite = CMTime {
            value: 0,
            timescale: 1,
            flags: CMTimeFlags::Valid.union(CMTimeFlags::Indefinite),
            epoch: 0,
        };
        let invalid = CMTime {
            value: 1,
            timescale: 1,
            flags: CMTimeFlags::empty(),
            epoch: 0,
        };
        let negative = CMTime {
            value: -1,
            timescale: 1,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        assert_eq!(super::duration_from(indefinite), None);
        assert_eq!(super::duration_from(invalid), None);
        assert_eq!(super::duration_from(negative), None);
    }

    /// Each `timeControlStatus` maps to the state the observation table names.
    #[test]
    fn time_control_status_maps_to_playback_state() {
        assert_eq!(
            super::state_for(AVPlayerTimeControlStatus::Playing),
            PlaybackState::Playing
        );
        assert_eq!(
            super::state_for(AVPlayerTimeControlStatus::WaitingToPlayAtSpecifiedRate),
            PlaybackState::Buffering
        );
        assert_eq!(
            super::state_for(AVPlayerTimeControlStatus::Paused),
            PlaybackState::Paused
        );
    }

    /// `CGSizeZero` — an unready item, and every audio-only one — publishes
    /// nothing.
    #[test]
    fn a_zero_presentation_size_publishes_nothing() {
        assert!(super::video_size_event(0.0, 0.0).is_none());
        assert!(super::video_size_event(1920.0, 0.0).is_none());
        assert!(super::video_size_event(f64::NAN, 1080.0).is_none());
    }

    /// A real geometry publishes the rounded pixel pair.
    #[test]
    fn a_real_presentation_size_publishes_its_pixels() {
        assert_eq!(
            super::video_size_event(1920.4, 1080.6),
            Some(PlayerEvent::VideoSize {
                width: 1920,
                height: 1081,
            })
        );
    }

    /// Looping is the `.none` action; not looping is `.pause`.
    #[test]
    fn looping_picks_the_action_at_item_end() {
        assert_eq!(
            super::action_at_item_end(true),
            AVPlayerActionAtItemEnd::None
        );
        assert_eq!(
            super::action_at_item_end(false),
            AVPlayerActionAtItemEnd::Pause
        );
    }

    /// Ids start at `1` and stay positive, so `0` is never a live session.
    #[test]
    fn session_ids_are_positive() {
        let id = super::next_session_id().expect("the id space is not exhausted in a test");
        assert!(id > 0);
    }
}
