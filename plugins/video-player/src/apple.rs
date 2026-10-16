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
//! and together they are the contract the iOS and macOS view factories build on:
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
//! # The accessor contract (frozen — both view factories depend on it)
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
//! # What the view factories can assume this module publishes
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
//! # Texture output
//!
//! A session opened with [`crate::VideoOutput::Texture`] gets a
//! [`TextureSink`] at open ([`AppleSession::texture_sink`]) and, once
//! [`construct`] has built the item, an `AVPlayerItemVideoOutput` attached to
//! it — asked for `32BGRA`, IOSurface-backed, Metal-compatible pixel buffers,
//! the one layout the renderer's `CVPixelBuffer` importer takes. No
//! `AVPlayerLayer` is involved: the output decodes for itself, on macOS and
//! iOS alike, with or without a view.
//!
//! A [`FrameClock`] — a `libdispatch` timer on the session's own serial frame
//! queue, never the main thread — polls `hasNewPixelBufferForItemTime:` at
//! twice a 60 Hz display rate while frames keep coming, and backs off to a
//! slow poll once the picture has been still for half a second (paused,
//! stalled, ended). Every state edge that can bring a new frame (ready to
//! play, playing, a seek landing) kicks it back to the fast rate. Each new
//! buffer is pushed into the sink:
//!
//! - **zero-copy** when the renderer has a `CVPixelBuffer` importer
//!   installed: a [`FramePayload::Native`] frame over the buffer itself, whose
//!   release callback drops the reference `copyPixelBufferForItemTime:` handed
//!   over (the importer takes its own for the texture's lifetime);
//! - **CPU copy** otherwise (no importer, or a build without the `frust-api`
//!   half): the buffer's rows swizzled into a tightly packed RGBA frame, the
//!   buffer released at once.
//!
//! The path is decided per frame and logged at `info!` whenever it changes,
//! together with a periodic frame-rate and buffer-pool line, so a profile
//! build's log shows which one ran. The output's own buffer pool sizes itself
//! to what is in flight (the bus holds at most a shown frame, the retire
//! backlog and the mailbox's pending one); nothing here ever waits on it.
//!
//! [`crate::PlayerSession::close`] marks the target closed and releases the
//! frame still pending; the teardown then cancels the clock and removes the
//! output from the item before the item itself is let go.
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
//! check. The texture output adds the `CoreVideo` calls on the pixel buffers
//! it is handed, the `unsafe` [`NativeHandle::new`] it builds frames with, the
//! dispatch timer behind [`FrameClock`], and one more thread-safety assertion
//! ([`PixelOutput`]).
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

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{
    _dispatch_source_type_timer, DispatchObject, DispatchQueue, DispatchQueueAttr,
    DispatchRetained, DispatchSource, DispatchTime,
};
use frust_plugin::texture::{
    FrameMeta, FramePayload, NativeHandle, NativeHandleKind, TextureFormat, TextureFrame,
    TextureSink,
};
use objc2::encode::{Encoding, RefEncode};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_av_foundation::{
    AVError, AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem,
    AVPlayerItemDidPlayToEndTimeNotification, AVPlayerItemStatus, AVPlayerTimeControlStatus,
};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNotification,
    NSNotificationCenter, NSNumber, NSObjectNSKeyValueObserverRegistration, NSString, NSURL,
};

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlaybackState, PlayerEvent, PlayerOptions, VideoError, VideoOutput, VideoSource};

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
    /// Where a [`VideoOutput::Texture`] session's frames go — handed to the
    /// [`FrameClock`] [`construct`] starts (module doc's *Texture output*).
    texture: Option<Arc<TextureTarget>>,
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
    /// The texture output's frame clock (and through it the
    /// `AVPlayerItemVideoOutput` attached to `item`), for a
    /// [`VideoOutput::Texture`] session whose output could be built.
    frame_clock: Option<Arc<FrameClock>>,
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
        let texture = (options.output == VideoOutput::Texture).then(|| {
            Arc::new(TextureTarget {
                session: id,
                sink: TextureSink::new(),
                closed: AtomicBool::new(false),
            })
        });

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
                texture: texture.clone(),
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

        Ok(AppleSession { id, texture })
    }
}

/// One AVFoundation player session.
///
/// Holds its id, plus the frame target of a texture session: everything else
/// lives in [`SESSIONS`], because an observer, a block, and [`player_for`] all
/// reach the same state from a bare integer and none of them has a handle to
/// this struct. Both fields are `Send + Sync`, as [`BackendSession`] requires.
pub(crate) struct AppleSession {
    /// The id the params payload carries and [`player_for`] resolves.
    id: i32,
    /// The mailbox and closed flag of a [`VideoOutput::Texture`] session
    /// (module doc's *Texture output*); `None` for a native-view session.
    texture: Option<Arc<TextureTarget>>,
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
    ///
    /// A texture session stops pushing here, on the calling thread, and the
    /// frame still pending in its mailbox is released rather than shown; the
    /// clock and the output themselves go with the main-thread teardown.
    fn close(&self) {
        if let Some(target) = &self.texture {
            target.close();
        }
        let id = self.id;
        on_main(move |mtm| teardown(id, mtm));
    }

    fn id(&self) -> u32 {
        // Ids are minted positive by `next_session_id`, which refuses to hand
        // out anything else, so this conversion never loses one.
        u32::try_from(self.id).unwrap_or(0)
    }

    fn texture_sink(&self) -> Option<&TextureSink> {
        self.texture.as_deref().map(|target| &target.sink)
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
/// that gets `None` should attach nothing, remember the view, and let the
/// `on_player_ready` call [`construct`] makes once the player is registered (or
/// a later update) attach it, rather than treat it as an error.
///
/// Callable only on the main thread: an `AVPlayerLayer` could not be attached
/// from anywhere else, so an off-thread call is a caller bug — logged and
/// answered `None` rather than asserted.
// The view factories are its only callers; this is the frozen
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
    /// A texture session's frame clock, kicked back to its fast rate by every
    /// edge that can bring a new frame (module doc's *Texture output*).
    frame_clock: Option<Arc<FrameClock>>,
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
        frame_clock: objects.frame_clock.clone(),
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

    let Some((volume, looping, want_play, rate, texture)) = update_session(session, |entry| {
        (
            entry.volume,
            entry.looping,
            entry.want_play,
            entry.rate,
            entry.texture.clone(),
        )
    }) else {
        return;
    };

    // Attached as soon as the item exists, so the output is in place before
    // the item can become ready and decode its first frame (module doc's
    // *Texture output*).
    let frame_clock = texture.and_then(|target| attach_texture_output(&item, target));

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
            frame_clock: frame_clock.clone(),
        });
    });
    if registered.is_none() {
        // Closed while the graph was being built: no observer is attached
        // yet, so stopping the frame clock (if one started) and dropping the
        // locals here is the whole teardown.
        if let Some(clock) = frame_clock {
            detach_texture_output(&item, &clock);
        }
        return;
    }

    attach_observers(session, &player, &item, &observer, mtm);

    // Any native view a slot created for this session before the player
    // existed has been waiting for exactly this moment (each factory's
    // *failure contract*); nothing else would ever re-invoke the factory for
    // a slot whose params did not change.
    #[cfg(target_os = "ios")]
    crate::ios_view::on_player_ready(session, mtm);
    #[cfg(target_os = "macos")]
    crate::macos_view::on_player_ready(session, mtm);

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

    // No player will ever be ready for a view still waiting on this session.
    #[cfg(target_os = "ios")]
    crate::ios_view::on_session_closed(session);
    #[cfg(target_os = "macos")]
    crate::macos_view::on_session_closed(session);

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
    // The frame clock first: it polls the item's output from its own queue,
    // and must be cancelled before the output leaves the item.
    if let Some(clock) = objects.frame_clock.as_ref() {
        detach_texture_output(&objects.item, clock);
    }

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
        // A paused item shows its first frame now, so a texture session's
        // clock polls for it at the fast rate.
        kick_frame_clock(&live);
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
    if state == PlaybackState::Playing {
        kick_frame_clock(&live);
    }

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
    // A seek lands on a new frame even while paused.
    kick_frame_clock(&live);
    live.shared
        .publish(position_event(&live.player, &live.item));
}

/// Return a texture session's frame clock to its fast rate (module doc's
/// *Texture output*); nothing for a native-view session.
fn kick_frame_clock(live: &Live) {
    if let Some(clock) = &live.frame_clock {
        clock.kick();
    }
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

// --- Texture output ---------------------------------------------------------------

/// The label of a texture session's frame queue (module doc's *Texture
/// output*) — one per session, never the main queue.
const FRAMES_QUEUE_LABEL: &str = "dev.frust.videoplayer.frames";

/// `kCVPixelFormatType_32BGRA` (`'BGRA'`): the layout the output is asked for,
/// and the one the renderer's `CVPixelBuffer` importer takes.
const PIXEL_FORMAT_32BGRA: u32 = u32::from_be_bytes(*b"BGRA");

/// `kCVPixelBufferLock_ReadOnly`, the CPU-copy path's lock flag.
const PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 1;

/// The fast poll: twice a 60 Hz display rate, so a 60 fps clip never loses a
/// frame to beating between its cadence and the poll's.
const FAST_POLL: Duration = Duration::from_nanos(8_333_333);
/// The fast poll's leeway: tight, so frames are not bunched.
const FAST_LEEWAY: Duration = Duration::from_millis(1);
/// The poll once the picture has been still for a while: a frame no state
/// edge announced (a stall clearing) still shows within a fifteenth of a
/// second, and then the fast poll resumes.
const SLOW_POLL: Duration = Duration::from_nanos(66_666_667);
/// The slow poll's leeway.
const SLOW_LEEWAY: Duration = Duration::from_millis(10);
/// Fast polls in a row without a new frame before the clock backs off to
/// [`SLOW_POLL`] — half a second.
const IDLE_POLLS_BEFORE_BACKOFF: u32 = 60;
/// How far past "now" a poll asks for a frame: one 60 Hz display frame, the
/// earliest the renderer presents a frame it takes now.
const DISPLAY_LOOKAHEAD_SECS: f64 = 1.0 / 60.0;
/// The delivered-frame count of the first rate/pool log line.
const STATS_FIRST_AT: u64 = 240;
/// Delivered frames between later rate/pool log lines.
const STATS_EVERY: u64 = 3600;
/// The most distinct backing surfaces the pool statistic tracks — far above a
/// steady pool, so reaching it reads as "the pool keeps growing".
const MAX_TRACKED_BUFFERS: usize = 64;

/// The `__CVBuffer` a `CVPixelBufferRef` points at, declared here because this
/// crate does not depend on `objc2-core-video`. Never constructed or
/// dereferenced on this side — only its address travels.
#[repr(C)]
struct CVBufferOpaque {
    _private: [u8; 0],
}

// SAFETY: `__CVBuffer` is the struct CoreVideo's headers (and
// `objc2-core-video`'s own binding) declare a `CVPixelBufferRef` to point at,
// so a pointer to this type encodes exactly as the runtime's method
// signatures name the buffer.
unsafe impl RefEncode for CVBufferOpaque {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
}

/// A `CVPixelBufferRef`.
type CVPixelBufferRef = *mut CVBufferOpaque;

// The CoreVideo pixel-buffer calls and attribute keys the texture output
// uses; the keys are `CFStringRef`s.
#[allow(non_upper_case_globals)]
#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: *const c_void;
    static kCVPixelBufferIOSurfacePropertiesKey: *const c_void;
    static kCVPixelBufferMetalCompatibilityKey: *const c_void;
    fn CVPixelBufferRelease(buffer: CVPixelBufferRef);
    fn CVPixelBufferGetWidth(buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetHeight(buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetPixelFormatType(buffer: CVPixelBufferRef) -> u32;
    fn CVPixelBufferGetIOSurface(buffer: CVPixelBufferRef) -> *const c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetBaseAddress(buffer: CVPixelBufferRef) -> *mut c_void;
    fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
}

// The host clock `itemTimeForHostTime:` is expressed in.
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    fn CACurrentMediaTime() -> f64;
}

/// The frame side of one [`VideoOutput::Texture`] session, shared by
/// [`AppleSession`] (which hands the mailbox out and closes it) and the
/// session's [`FrameClock`] (which pushes into it).
struct TextureTarget {
    /// The session the frames belong to, for log lines.
    session: i32,
    /// The mailbox [`AppleSession::texture_sink`] hands out.
    sink: TextureSink,
    /// Set by [`AppleSession::close`]; no frame stays pending after it.
    closed: AtomicBool,
}

impl TextureTarget {
    /// Stop accepting frames and release the one still pending, if any.
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(frame) = self.sink.take_latest() {
            frame.release();
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Push `frame`, unless the session closed. A close landing while the push
    /// runs is caught by the second check, which takes the frame straight back
    /// out: either that check or [`Self::close`]'s own drain releases it, so no
    /// frame stays pending past a close.
    fn push(&self, frame: TextureFrame) {
        if self.is_closed() {
            frame.release();
            return;
        }
        self.sink.push(frame);
        if self.is_closed()
            && let Some(frame) = self.sink.take_latest()
        {
            frame.release();
        }
    }
}

/// A session's `AVPlayerItemVideoOutput`, reached by message send: this crate
/// does not enable `objc2-av-foundation`'s `AVPlayerItemOutput` feature, and
/// the four messages the texture output needs are spelled out here instead.
struct PixelOutput {
    output: Retained<AnyObject>,
}

// SAFETY: `AVPlayerItemVideoOutput` is not main-thread-only (its binding
// carries no `MainThreadOnly` kind, unlike `AVPlayer`), and polling it —
// `hasNewPixelBufferForItemTime:`/`copyPixelBufferForItemTime:` — from a
// display-link or render thread is its documented use (Apple's own samples
// poll it from a `CVDisplayLink` thread). Here the main thread only adds it to
// and removes it from its item, and the session's serial frame queue only
// polls it; retain/release is thread-safe on any Objective-C object.
unsafe impl Send for PixelOutput {}
// SAFETY: as above — a shared `&PixelOutput` only ever polls.
unsafe impl Sync for PixelOutput {}

impl PixelOutput {
    /// An output producing `32BGRA`, IOSurface-backed, Metal-compatible pixel
    /// buffers, or `None` if the class or its initializer answered nil.
    fn new() -> Option<Self> {
        let class = AnyClass::get(c"AVPlayerItemVideoOutput")?;
        let attributes = pixel_buffer_attributes();
        // SAFETY: `+alloc` on a live class, then its designated initializer
        // with an `NSDictionary<NSString *, id>` of CoreVideo attribute keys —
        // the argument type `-initWithPixelBufferAttributes:` declares.
        let output: Option<Retained<AnyObject>> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![allocated, initWithPixelBufferAttributes: &*attributes]
        };
        output.map(|output| Self { output })
    }

    /// The item time of the frame the renderer would present next: now plus
    /// one display frame.
    fn item_time_for_display(&self) -> CMTime {
        // SAFETY: `CACurrentMediaTime` is a plain clock read, and
        // `itemTimeForHostTime:` takes a `CFTimeInterval` in that clock's
        // timebase and returns a `CMTime` by value.
        unsafe {
            let host = CACurrentMediaTime() + DISPLAY_LOOKAHEAD_SECS;
            msg_send![&*self.output, itemTimeForHostTime: host]
        }
    }

    /// Whether a frame for `time` arrived since the last copy.
    fn has_new_buffer(&self, time: CMTime) -> bool {
        // SAFETY: a query taking a `CMTime` by value and returning a `BOOL`.
        unsafe { msg_send![&*self.output, hasNewPixelBufferForItemTime: time] }
    }

    /// The buffer for `time`, at +1 (the method is in the `copy` family): the
    /// caller owns one reference and gives it back with
    /// `CVPixelBufferRelease`. Null when there is none.
    fn copy_buffer(&self, time: CMTime) -> CVPixelBufferRef {
        // SAFETY: takes a `CMTime` by value and an optional `CMTime *` out
        // parameter (null: the display time is not wanted), and returns a
        // `CVPixelBufferRef` the caller owns.
        unsafe {
            msg_send![
                &*self.output,
                copyPixelBufferForItemTime: time,
                itemTimeForDisplay: ptr::null_mut::<CMTime>()
            ]
        }
    }
}

/// `{ PixelFormatType: 32BGRA, IOSurfaceProperties: {}, MetalCompatibility:
/// YES }` — what the renderer's importer needs: an IOSurface-backed `32BGRA`
/// buffer can alias a Metal texture.
fn pixel_buffer_attributes() -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: reading `extern` CoreVideo constant statics (edition-2024
    // unsafe). Each is a linker-provided, non-null `CFStringRef` that lives
    // for the whole process, and a `CFString` is toll-free bridged to
    // `NSString`, so viewing it as one is sound.
    let (format_key, surface_key, metal_key) = unsafe {
        (
            &*kCVPixelBufferPixelFormatTypeKey.cast::<NSString>(),
            &*kCVPixelBufferIOSurfacePropertiesKey.cast::<NSString>(),
            &*kCVPixelBufferMetalCompatibilityKey.cast::<NSString>(),
        )
    };
    let format = NSNumber::numberWithUnsignedInt(PIXEL_FORMAT_32BGRA);
    let surface = NSDictionary::<NSString, AnyObject>::new();
    let metal = NSNumber::numberWithBool(true);
    let (format, surface, metal): (&AnyObject, &AnyObject, &AnyObject) =
        (&format, &surface, &metal);
    NSDictionary::from_slices(
        &[format_key, surface_key, metal_key],
        &[format, surface, metal],
    )
}

/// Which way a frame reached the mailbox — logged whenever it changes.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum FramePath {
    /// No frame delivered yet.
    None = 0,
    /// The buffer itself, imported by the renderer without a copy.
    ZeroCopy = 1,
    /// The buffer's rows copied into an RGBA frame.
    CpuCopy = 2,
    /// A buffer that could not be delivered at all.
    Refused = 3,
}

/// Delivery statistics behind the periodic log line.
struct FrameStats {
    /// Frames delivered since the session's first.
    frames: u64,
    /// When the current statistics window started, and its first frame count.
    window: Option<(Instant, u64)>,
    /// Distinct IOSurfaces seen behind the buffers in the current window, up
    /// to [`MAX_TRACKED_BUFFERS`]: the output may wrap each frame in a fresh
    /// `CVPixelBuffer` (it does on macOS), but during steady playback the
    /// surfaces holding the pixels cycle through a fixed pool, so this reads
    /// as the pool's size. A seek (a loop restart included) may bring fresh
    /// surfaces.
    surfaces: HashSet<usize>,
}

/// The texture output's poll (module doc's *Texture output*): a dispatch
/// timer on the session's own serial frame queue.
struct FrameClock {
    /// The timer. Its handler holds only a weak reference to this clock, so
    /// the two never keep each other alive; [`Self::stop`] (and the drop)
    /// cancel it.
    source: DispatchRetained<DispatchSource>,
    /// The output it polls.
    output: PixelOutput,
    /// Where the frames go.
    target: Arc<TextureTarget>,
    /// Whether the timer runs at [`FAST_POLL`] rather than [`SLOW_POLL`].
    fast: AtomicBool,
    /// Fast polls in a row that found no new frame.
    idle_polls: AtomicU32,
    /// The [`FramePath`] the last frame took.
    path: AtomicU8,
    /// Touched only from the serial frame queue, so never contended.
    stats: Mutex<FrameStats>,
}

impl FrameClock {
    /// Start polling `output` for `target`, at the fast rate.
    fn start(output: PixelOutput, target: Arc<TextureTarget>) -> Arc<Self> {
        let queue = DispatchQueue::new(FRAMES_QUEUE_LABEL, DispatchQueueAttr::SERIAL);
        // SAFETY: the timer source type with the handle and mask a timer
        // takes (both zero), targeting a live serial queue; the source
        // retains the queue.
        let source = unsafe {
            DispatchSource::new(
                (&raw const _dispatch_source_type_timer).cast_mut(),
                0,
                0,
                Some(&queue),
            )
        };
        let clock = Arc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let handler = RcBlock::new(move || {
                // Called from a dispatch frame: nothing may unwind into it.
                let Some(clock) = weak.upgrade() else {
                    return;
                };
                if catch_unwind(AssertUnwindSafe(|| clock.tick())).is_err() {
                    log::error!("frust-video-player: panic caught in the texture frame clock");
                }
            });
            // SAFETY: a live block of dispatch's `void (^)(void)` handler
            // shape; dispatch copies it, so its lifetime past this call is the
            // source's business.
            unsafe { source.set_event_handler_with_block(RcBlock::as_ptr(&handler)) };
            Self {
                source,
                output,
                target,
                fast: AtomicBool::new(true),
                idle_polls: AtomicU32::new(0),
                path: AtomicU8::new(FramePath::None as u8),
                stats: Mutex::new(FrameStats {
                    frames: 0,
                    window: None,
                    surfaces: HashSet::new(),
                }),
            }
        });
        clock
            .source
            .set_timer(DispatchTime::NOW, nanos(FAST_POLL), nanos(FAST_LEEWAY));
        clock.source.resume();
        clock
    }

    /// Back to the fast rate — an edge that can bring a new frame happened.
    fn kick(&self) {
        self.idle_polls.store(0, Ordering::Relaxed);
        self.set_fast(true);
    }

    /// Stop polling. Idempotent; a handler already running finishes, but
    /// pushes nothing once the session's target is closed.
    fn stop(&self) {
        self.source.cancel();
    }

    fn set_fast(&self, fast: bool) {
        if self.fast.swap(fast, Ordering::AcqRel) == fast {
            return;
        }
        let (interval, leeway) = if fast {
            (FAST_POLL, FAST_LEEWAY)
        } else {
            (SLOW_POLL, SLOW_LEEWAY)
        };
        self.source
            .set_timer(DispatchTime::NOW, nanos(interval), nanos(leeway));
    }

    /// One poll: copy the frame due next, if a new one arrived, and push it.
    fn tick(&self) {
        if self.target.is_closed() {
            return;
        }
        let time = self.output.item_time_for_display();
        if !self.output.has_new_buffer(time) {
            let idle = self
                .idle_polls
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1);
            if idle == IDLE_POLLS_BEFORE_BACKOFF {
                self.set_fast(false);
            }
            return;
        }
        self.idle_polls.store(0, Ordering::Relaxed);
        self.set_fast(true);
        let buffer = self.output.copy_buffer(time);
        if !buffer.is_null() {
            self.deliver(buffer);
        }
    }

    /// Push `buffer` — owned at +1 — into the mailbox, zero-copy when the
    /// renderer can import it, as an RGBA copy otherwise.
    fn deliver(&self, buffer: CVPixelBufferRef) {
        // SAFETY: `buffer` is the live, non-null pixel buffer `copy_buffer`
        // just returned, on which this call owns one reference. The surface
        // is only an address for the pool statistic (a Get-rule answer, never
        // dereferenced or released here).
        let (width, height, format, surface) = unsafe {
            (
                CVPixelBufferGetWidth(buffer),
                CVPixelBufferGetHeight(buffer),
                CVPixelBufferGetPixelFormatType(buffer),
                CVPixelBufferGetIOSurface(buffer) as usize,
            )
        };
        let address = buffer as usize;
        let extent = u32::try_from(width).ok().zip(u32::try_from(height).ok());
        let Some((width, height)) = extent.filter(|(w, h)| *w > 0 && *h > 0) else {
            release_pixel_buffer(address);
            return;
        };
        if format != PIXEL_FORMAT_32BGRA {
            self.note_path(FramePath::Refused, width, height, format);
            release_pixel_buffer(address);
            return;
        }

        let path = if zero_copy_available() {
            // SAFETY: `buffer` is a live `CVPixelBufferRef` on which this
            // frame owns the reference `copyPixelBufferForItemTime:` handed
            // over; only the release callback attached below drops it, and a
            // frame's release runs exactly once.
            let handle =
                unsafe { NativeHandle::new(NativeHandleKind::CvPixelBuffer, buffer.cast()) };
            let meta = FrameMeta {
                width,
                height,
                format: TextureFormat::Rgba8,
            };
            self.target.push(
                TextureFrame::new(meta, FramePayload::Native(handle))
                    .on_release(move || release_pixel_buffer(address)),
            );
            FramePath::ZeroCopy
        } else {
            let pixels = copy_bgra_buffer(buffer, width, height);
            release_pixel_buffer(address);
            match pixels {
                Some(pixels) => {
                    self.target.push(TextureFrame::rgba8(width, height, pixels));
                    FramePath::CpuCopy
                }
                None => FramePath::Refused,
            }
        };
        self.note_path(path, width, height, format);
        self.record(surface);
    }

    /// Log the delivery path whenever it changes — the line a device run
    /// reads to tell the zero-copy path from the copy.
    fn note_path(&self, path: FramePath, width: u32, height: u32, format: u32) {
        if self.path.swap(path as u8, Ordering::Relaxed) == path as u8 {
            return;
        }
        let session = self.target.session;
        match path {
            FramePath::ZeroCopy => log::info!(
                "frust-video-player: session {session} texture output via CVPixelBuffer \
                 (zero-copy importer installed), {width}x{height} 32BGRA"
            ),
            FramePath::CpuCopy => log::info!(
                "frust-video-player: session {session} texture output via CPU copy (no \
                 CVPixelBuffer importer installed), {width}x{height}"
            ),
            FramePath::Refused => log::warn!(
                "frust-video-player: session {session} texture output dropped a {width}x{height} \
                 buffer of pixel format {} (32BGRA expected, or it could not be read)",
                fourcc(format)
            ),
            FramePath::None => {}
        }
    }

    /// Count one delivered frame and log the rate and pool size every so
    /// often (module doc's *Texture output*).
    fn record(&self, surface: usize) {
        let mut stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        stats.frames += 1;
        let frames = stats.frames;
        let (window_start, window_first) = *stats.window.get_or_insert((now, frames));
        if surface != 0 && stats.surfaces.len() < MAX_TRACKED_BUFFERS {
            stats.surfaces.insert(surface);
        }
        let due = frames == STATS_FIRST_AT
            || (frames > STATS_FIRST_AT && (frames - STATS_FIRST_AT).is_multiple_of(STATS_EVERY));
        if !due {
            return;
        }
        let window_frames = frames - window_first;
        let secs = now.duration_since(window_start).as_secs_f64();
        let fps = if secs > 0.0 {
            window_frames as f64 / secs
        } else {
            0.0
        };
        let path = match self.path.load(Ordering::Relaxed) {
            1 => "zero-copy",
            2 => "CPU copy",
            _ => "mixed",
        };
        log::info!(
            "frust-video-player: session {} texture output: {frames} frames, {fps:.1} fps over the \
             last {secs:.1}s, {} distinct IOSurfaces in that window, {path}",
            self.target.session,
            stats.surfaces.len()
        );
        stats.window = Some((now, frames));
        stats.surfaces.clear();
    }
}

impl Drop for FrameClock {
    /// A clock dropped without [`Self::stop`] still stops its timer, which
    /// would otherwise keep firing at a clock that is gone.
    fn drop(&mut self) {
        self.source.cancel();
    }
}

/// Attach a texture output to `item` and start its clock — [`construct`]'s
/// texture half, on the main thread. `None` (logged) when the output could
/// not be built; the session then plays with an empty mailbox.
fn attach_texture_output(
    item: &AVPlayerItem,
    target: Arc<TextureTarget>,
) -> Option<Arc<FrameClock>> {
    let session = target.session;
    let Some(output) = PixelOutput::new() else {
        log::warn!(
            "frust-video-player: session {session} could not create an AVPlayerItemVideoOutput \
             — no texture frames"
        );
        return None;
    };
    // SAFETY: `addOutput:` on a live item, on the main thread, with a freshly
    // built `AVPlayerItemVideoOutput` — an `AVPlayerItemOutput`, the argument
    // type it declares. Balanced by `removeOutput:` in
    // `detach_texture_output`.
    unsafe {
        let _: () = msg_send![item, addOutput: &*output.output];
    }
    log::info!(
        "frust-video-player: session {session} texture output attached \
         (AVPlayerItemVideoOutput, 32BGRA, IOSurface-backed)"
    );
    Some(FrameClock::start(output, target))
}

/// Stop `clock` and take its output back off `item`, on the main thread.
fn detach_texture_output(item: &AVPlayerItem, clock: &FrameClock) {
    clock.stop();
    // SAFETY: `removeOutput:` on a live item, on the main thread, with the
    // output `attach_texture_output` added to that same item.
    unsafe {
        let _: () = msg_send![item, removeOutput: &*clock.output.output];
    }
}

/// Whether the renderer can import `CVPixelBuffer`s right now. A native frame
/// of a kind with no importer installed is refused by the frame bus, so
/// without one (or without the `frust-api` half, which has no renderer to ask)
/// frames are copied.
fn zero_copy_available() -> bool {
    #[cfg(feature = "frust-api")]
    {
        frust::texture::native_importer_installed(frust::texture::NativeHandleKind::CvPixelBuffer)
    }
    #[cfg(not(feature = "frust-api"))]
    {
        false
    }
}

/// Drops one reference to a pixel buffer. A zero-copy frame's release
/// callback, so it must not panic: a single CoreVideo call.
fn release_pixel_buffer(address: usize) {
    // SAFETY: `address` is a `CVPixelBufferRef` carrying one reference owned
    // by its caller — a frame's, released exactly once by its callback, or
    // `deliver`'s own on a path that hands no frame out.
    // `CVPixelBufferRelease` is thread-safe.
    unsafe { CVPixelBufferRelease(address as CVPixelBufferRef) };
}

/// The CPU path's copy: lock `buffer` read-only and swizzle its `32BGRA` rows
/// into tightly packed RGBA. `None` when it cannot be locked or read.
fn copy_bgra_buffer(buffer: CVPixelBufferRef, width: u32, height: u32) -> Option<Arc<[u8]>> {
    // SAFETY: a read-only lock on a live buffer this call holds a reference
    // to, unlocked below on every path.
    if unsafe { CVPixelBufferLockBaseAddress(buffer, PIXEL_BUFFER_LOCK_READ_ONLY) } != 0 {
        return None;
    }
    // SAFETY: the buffer is locked, so its base address and row stride are
    // valid, and `stride * height` bytes from that address are readable until
    // the unlock below; the slice does not outlive it.
    let pixels = unsafe {
        let base = CVPixelBufferGetBaseAddress(buffer);
        let stride = CVPixelBufferGetBytesPerRow(buffer);
        let len = stride.checked_mul(height as usize);
        match (base.is_null(), len) {
            (false, Some(len)) => {
                let bytes = std::slice::from_raw_parts(base.cast::<u8>().cast_const(), len);
                bgra_rows_to_rgba(bytes, width, height, stride)
            }
            _ => None,
        }
    };
    // SAFETY: the matching unlock, with the flags the lock took.
    unsafe { CVPixelBufferUnlockBaseAddress(buffer, PIXEL_BUFFER_LOCK_READ_ONLY) };
    pixels
}

/// Swizzles `height` rows of `32BGRA` pixels, `stride` bytes apart, into a
/// tightly packed RGBA buffer. `None` when a row cannot hold `width` pixels or
/// `src` cannot hold `height` rows. The output's frames are opaque, so the
/// result is premultiplied as it stands.
fn bgra_rows_to_rgba(src: &[u8], width: u32, height: u32, stride: usize) -> Option<Arc<[u8]>> {
    let row = (width as usize).checked_mul(4)?;
    let rows = height as usize;
    if stride < row || rows == 0 || src.len() < stride.checked_mul(rows - 1)?.checked_add(row)? {
        return None;
    }
    let mut out = Vec::with_capacity(row * rows);
    for line in src.chunks(stride).take(rows) {
        for &[b, g, r, a] in line[..row].as_chunks::<4>().0 {
            out.extend_from_slice(&[r, g, b, a]);
        }
    }
    Some(Arc::from(out))
}

/// A CoreVideo pixel-format code as its four characters (`'420v'`), or hex
/// when they are not printable.
fn fourcc(code: u32) -> String {
    let bytes = code.to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        format!("'{}'", String::from_utf8_lossy(&bytes))
    } else {
        format!("{code:#010x}")
    }
}

/// A [`Duration`] in whole nanoseconds, as `libdispatch` takes it.
fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
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

/// Schemes a [`VideoSource::Url`] may name — every other scheme is refused by
/// [`resolve_source`] before `NSURL` ever sees the string. A media URL is the
/// app data most likely to arrive from outside the trust boundary (a CMS
/// response, a deep link, a scanned QR code), and `file://`, `content://`,
/// `data:` and the like each reach somewhere neither this crate nor
/// `AVPlayer` chose to expose.
const URL_SCHEMES: [&str; 2] = ["http", "https"];

/// The scheme naming `url`, ASCII-case-insensitively — the substring before
/// its first `:` — or `None` when `url` has no `:` at all.
fn url_scheme(url: &str) -> Option<&str> {
    url.split_once(':').map(|(scheme, _)| scheme)
}

/// Resolve a [`VideoSource`] to something an `NSURL` can be built from, on the
/// calling thread.
///
/// # Errors
/// [`VideoError::UnsupportedSource`] for a relative or non-UTF-8 file path
/// (matching the Android backend, which refuses both for the same reason — the
/// platform resolves nothing, so a relative path would be read against whatever
/// working directory the process happens to have), an asset the main bundle does
/// not contain or that a containment check ([`bundle_resource_path`]) would
/// resolve outside it, a [`VideoSource::Url`] whose scheme is not in
/// [`URL_SCHEMES`] (message names only the refused scheme, never the full
/// URL — a refused URL is exactly the input least safe to repeat verbatim),
/// or a URL Foundation will not parse.
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
            match url_scheme(url) {
                Some(scheme)
                    if URL_SCHEMES
                        .iter()
                        .any(|allowed| scheme.eq_ignore_ascii_case(allowed)) => {}
                Some(scheme) => {
                    return Err(VideoError::UnsupportedSource(format!(
                        "unsupported URL scheme `{scheme}`"
                    )));
                }
                None => {
                    return Err(VideoError::UnsupportedSource(
                        "a URL source must have a scheme".to_owned(),
                    ));
                }
            }
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

/// `[[NSBundle mainBundle] resourcePath]` joined with `name`, if the result
/// stays inside the bundle's resource directory and the file is actually
/// there.
///
/// `NSBundle` is reached through a runtime class lookup rather than the
/// `objc2-foundation` binding on purpose: the binding sits behind that crate's
/// `NSBundle` feature, which this crate's manifest does not enable, and widening
/// a shared feature list for one call is a worse trade than four lines of
/// `msg_send!`. The class is part of Foundation itself, so the lookup fails only
/// in a process with no Foundation loaded — where nothing else in this module
/// would work either.
///
/// Containment is checked by [`contained_resource_path`] before existence is
/// checked with the standard library rather than `pathForResource:ofType:`,
/// because a bundled asset is named by *path* (`clips/intro.mp4`), which that
/// API's name/extension split does not model.
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

    let path = contained_resource_path(Path::new(&resources), name)?;
    if !path.is_file() {
        return None;
    }
    path.to_str().map(str::to_owned)
}

/// Whether `name`, joined onto `base`, stays inside `base` — an asset name is
/// untrusted app data (the crate doc's threat model: a CMS response, a deep
/// link, a QR code), so this refuses an absolute name and any `..` or other
/// non-[`Component::Normal`] component before ever touching the filesystem,
/// then canonicalizes the joined path and requires it to still start with
/// `base`'s own canonical form — catching a symlink inside `base` that walks
/// back out, not just a `..` spelled in `name`. `None` for a name that fails
/// either check, or for a `base`/joined path that does not exist at all,
/// since canonicalizing needs a real path.
///
/// Pure and filesystem-only (no Objective-C), so it is unit-testable on any
/// host with a stubbed `base`.
fn contained_resource_path(base: &Path, name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return None;
    }
    if candidate
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }

    let canonical_base = base.canonicalize().ok()?;
    let canonical_joined = base.join(candidate).canonicalize().ok()?;
    if canonical_joined.starts_with(&canonical_base) {
        Some(canonical_joined)
    } else {
        None
    }
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

    use std::path::PathBuf;
    use std::time::Duration;

    use objc2_av_foundation::{AVPlayerActionAtItemEnd, AVPlayerTimeControlStatus};
    use objc2_core_media::{CMTime, CMTimeFlags};

    use super::{contained_resource_path, resolve_source, url_scheme};
    use crate::{PlaybackState, PlayerEvent, VideoError, VideoSource};

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

    /// [`url_scheme`] splits on the first `:` and reports `None` for a
    /// scheme-less string.
    #[test]
    fn url_scheme_splits_on_first_colon() {
        assert_eq!(url_scheme("https://example.test"), Some("https"));
        assert_eq!(url_scheme("HTTP://example.test"), Some("HTTP"));
        assert_eq!(url_scheme("no-scheme-here"), None);
        assert_eq!(url_scheme("data:text/plain,a:b"), Some("data"));
    }

    /// A scheme outside the `http`/`https` allowlist is refused, and the
    /// message names only the scheme, never the full (potentially sensitive)
    /// URL.
    #[test]
    fn resolve_source_refuses_a_disallowed_url_scheme() {
        for url in ["file:///x", "content://x", "data:text/plain,x", "rtmp://x"] {
            let refused = resolve_source(&VideoSource::Url(url.to_owned()));
            match refused {
                Err(VideoError::UnsupportedSource(message)) => {
                    assert!(
                        !message.contains(url),
                        "message `{message}` echoed the refused URL `{url}`"
                    );
                }
                Ok(_) => panic!("expected UnsupportedSource for `{url}`, got Ok"),
                Err(other) => panic!("expected UnsupportedSource for `{url}`, got {other:?}"),
            }
        }
    }

    /// A URL string with no scheme at all is refused too.
    #[test]
    fn resolve_source_refuses_a_schemeless_url() {
        let refused = resolve_source(&VideoSource::Url("example.test/a.mp4".to_owned()));
        match refused {
            Err(VideoError::UnsupportedSource(_)) => {}
            Ok(_) => panic!("expected UnsupportedSource, got Ok"),
            Err(other) => panic!("expected UnsupportedSource, got {other:?}"),
        }
    }

    /// A throwaway directory tree under the OS temp dir, unique to `unique`,
    /// for [`contained_resource_path`]'s tests to canonicalize against —
    /// canonicalizing needs a `base` that actually exists.
    fn stub_bundle_root(unique: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "frust-video-player-apple-test-{unique}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("clips")).expect("create stub bundle root");
        std::fs::write(root.join("clips").join("intro.mp4"), b"stub").expect("write stub asset");
        root
    }

    /// An absolute name is refused before the filesystem is ever touched.
    #[test]
    fn contained_resource_path_refuses_an_absolute_name() {
        let base = stub_bundle_root("absolute");
        assert_eq!(contained_resource_path(&base, "/etc/passwd"), None);
    }

    /// A leading `..` component is refused.
    #[test]
    fn contained_resource_path_refuses_a_parent_escape() {
        let base = stub_bundle_root("escape");
        assert_eq!(contained_resource_path(&base, "../escape"), None);
    }

    /// A name that dips below `base` and climbs back out further than it
    /// went in is refused too, not just a bare leading `..`.
    #[test]
    fn contained_resource_path_refuses_a_nested_escape() {
        let base = stub_bundle_root("nested-escape");
        assert_eq!(contained_resource_path(&base, "a/../../b"), None);
    }

    /// A valid nested name resolves to the canonicalized file inside `base`.
    #[test]
    fn contained_resource_path_accepts_a_valid_nested_name() {
        let base = stub_bundle_root("valid");
        let resolved =
            contained_resource_path(&base, "clips/intro.mp4").expect("nested name resolves");
        assert_eq!(
            resolved,
            base.canonicalize()
                .expect("stub base exists")
                .join("clips")
                .join("intro.mp4")
        );
    }

    /// A padded `32BGRA` image swizzles into tightly packed RGBA, row padding
    /// dropped.
    #[test]
    fn bgra_rows_swizzle_into_packed_rgba() {
        // 2x2 pixels, 12-byte stride (4 bytes of padding per row).
        let src = [
            1, 2, 3, 4, 5, 6, 7, 8, 0xEE, 0xEE, 0xEE, 0xEE, //
            9, 10, 11, 12, 13, 14, 15, 16,
        ];
        let rgba = super::bgra_rows_to_rgba(&src, 2, 2, 12).expect("a well-formed image");
        assert_eq!(
            &rgba[..],
            &[3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]
        );
    }

    /// A stride shorter than a row, or a source short of the last row, is
    /// refused rather than read past.
    #[test]
    fn malformed_bgra_images_are_refused() {
        assert!(super::bgra_rows_to_rgba(&[0; 16], 2, 2, 4).is_none());
        assert!(super::bgra_rows_to_rgba(&[0; 15], 2, 2, 8).is_none());
        assert!(super::bgra_rows_to_rgba(&[0; 16], 2, 0, 8).is_none());
    }

    /// Pixel-format codes read as their four characters, hex otherwise.
    #[test]
    fn pixel_formats_render_as_four_character_codes() {
        assert_eq!(super::fourcc(super::PIXEL_FORMAT_32BGRA), "'BGRA'");
        assert_eq!(super::fourcc(u32::from_be_bytes(*b"420v")), "'420v'");
        assert_eq!(super::fourcc(0x0000_0001), "0x00000001");
    }

    /// The poll intervals stay ordered and in nanoseconds.
    #[test]
    fn the_frame_clock_polls_faster_than_it_backs_off() {
        assert!(super::nanos(super::FAST_POLL) < super::nanos(super::SLOW_POLL));
        assert_eq!(super::nanos(Duration::from_millis(1)), 1_000_000);
    }
}
