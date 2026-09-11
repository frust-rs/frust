//! `frust-video-player`: one Rust video-playback API — open a file, a bundled
//! asset, or a URL; play/pause/seek/rate/volume/loop it; read its state; and
//! host its picture in a native platform-view slot — over the platform's own
//! player on each OS.
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-camera`](../frust_camera/index.html) and
//! [`frust-shared-preferences`](../frust_shared_preferences/index.html), this
//! is a **platform plugin** (see `docs/ARCHITECTURE.md`'s Module Structure):
//! its default build depends on `frust-plugin` plus FFI crates only, and
//! carries no other `frust-*` framework dependency. The app-facing half
//! ([`api`]) sits behind the default-on `frust-api` feature, so
//! `--no-default-features` still resolves to that charter line. An app adds
//! this crate to its own `Cargo.toml` alongside `frust`; the facade does not
//! depend on or re-export it.
//!
//! # The picture is a platform view, not a widget
//!
//! This crate never paints a frame itself. [`PlayerSession::view_type`]
//! returns the `viewType` string an app feeds straight into
//! `frust::platform_view`, and [`PlayerSession::params_json`] the params
//! payload that names *which* session the native view should attach to. The
//! native side of that slot is per-OS: a Kotlin factory on Android, and a
//! pure-Rust ObjC view factory on iOS and on macOS (no Swift glue, and no C
//! export — the Apple factories reach their player through a crate-private
//! accessor, not through an FFI symbol).
//!
//! Both the [`VIEW_TYPE`] constant and the params spelling are deliberately
//! **target-gated**, never one shared literal (`docs/CODE_STANDARDS.md`'s
//! Naming Conventions LAW): Android takes a fully-qualified class name under
//! `dev.frust.*` and spells the session key `sessionId`; Apple takes the
//! bare ObjC runtime name and spells it `session`.
//!
//! On macOS the hosted view is an **opaque native sibling** — the
//! platform-view Mode A contract, under which the frust slot paints nothing
//! and the OS composites the player's layer above the frust surface. Nothing
//! here requests a translucent surface, on macOS or anywhere else.
//!
//! # Threading: nothing blocks, and events arrive on the main thread
//!
//! Unlike `frust-camera`, **no call in this crate blocks** and none is
//! UI-thread-guarded: [`VideoPlayer::open`] hands the source to the platform
//! player and returns before it has loaded, and every control method
//! ([`PlayerSession::play`] and friends) is a command posted at the player,
//! not a wait on it. All of them are therefore safe to call from the UI
//! thread — which is the point, since that is where a frust app's event
//! handlers run.
//!
//! Outcomes come back the other way: the platform publishes them on its own
//! main thread, into this session's atomic snapshot
//! ([`PlayerSession::snapshot`], readable lock-free from any thread) and
//! then to the session's listener ([`PlayerSession::set_listener`]). A
//! listener therefore runs on the platform main thread and must never
//! block; a control call made from inside one is refused with
//! [`VideoError::Reentrant`] rather than re-entering the backend mid-event.
//!
//! # Backends
//!
//! [`android`] and [`apple`] (one AVFoundation session serving iOS **and**
//! macOS, with only the hosting view differing — [`ios_view`] vs
//! [`macos_view`]). On every other target — desktop Linux/Windows, wasm —
//! [`unsupported`] fails [`VideoPlayer::open`] soft with
//! [`VideoError::NotSupported`], never a panic, so an app can degrade
//! instead of crashing.
//!
//! Container/codec support is **not** symmetric across those backends and
//! this crate does not pretend otherwise: it forwards a source to the
//! platform player and reports what that player says. The one asymmetry
//! worth planning around up front is HLS (`.m3u8`), which the Apple player
//! handles natively while Android's needs its own streaming support — a
//! source that plays on one is not proof it plays on the other, so treat a
//! published [`VideoError::Decoder`] as a per-platform answer.

// Platform backends. Every target routes through `VideoPlayer::open`'s one
// selection point, which is a type alias (`backend::Active`) rather than a
// `cfg` in the function body.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(target_os = "ios")]
mod ios_view;
#[cfg(target_os = "macos")]
mod macos_view;
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
mod unsupported;

mod backend;
mod snapshot;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod mock;

#[cfg(feature = "frust-api")]
pub mod api;

use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use backend::{BackendSession, PlayerBackend};

/// The platform-view spellings this crate publishes — **all** of them, on
/// every target, so the host-side test suite can assert each one wherever it
/// runs. Exactly one of each pair is selected into [`VIEW_TYPE`]/
/// [`SESSION_KEY`] below per target; the others are unreferenced there by
/// construction, which is what the allowance covers.
#[allow(dead_code)]
mod contract {
    /// Android's `viewType`: a fully-qualified class under `dev.frust.*`,
    /// resolved by the embedding host through the app classloader.
    pub(crate) const ANDROID_VIEW_TYPE: &str = "dev.frust.videoplayer.VideoPlayerViewFactory";
    /// Apple's `viewType`: the bare ObjC runtime name, resolved on iOS via
    /// `NSClassFromString` and used on macOS as the desktop factory
    /// registry's key — one constant for both, because one factory name is
    /// what an app writes for both.
    pub(crate) const APPLE_VIEW_TYPE: &str = "VideoPlayerViewFactory";
    /// Android's params key naming the session to attach to.
    pub(crate) const ANDROID_SESSION_KEY: &str = "sessionId";
    /// Apple's params key naming the session to attach to.
    pub(crate) const APPLE_SESSION_KEY: &str = "session";
}

/// The `viewType` string identifying this crate's native video view — what
/// [`PlayerSession::view_type`] returns, published as a constant too so an
/// app can name the slot before it has a session.
#[cfg(target_os = "android")]
pub const VIEW_TYPE: &str = contract::ANDROID_VIEW_TYPE;

/// The `viewType` string identifying this crate's native video view — the
/// same name on iOS and macOS (see [`contract::APPLE_VIEW_TYPE`]).
#[cfg(target_vendor = "apple")]
pub const VIEW_TYPE: &str = contract::APPLE_VIEW_TYPE;

/// Empty on a target with no video backend: there is no native factory to
/// name, and [`VideoPlayer::open`] refuses before a session could ask for
/// one. An app may still read it — an empty `viewType` reserves no slot.
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
pub const VIEW_TYPE: &str = "";

/// The params key [`PlayerSession::params_json`] writes the session id
/// under. Android's factory reads `sessionId`; every other target's reads
/// `session` (on a target with no backend nothing reads it at all, since no
/// session can exist there).
#[cfg(target_os = "android")]
const SESSION_KEY: &str = contract::ANDROID_SESSION_KEY;
#[cfg(not(target_os = "android"))]
const SESSION_KEY: &str = contract::APPLE_SESSION_KEY;

/// What to play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSource {
    /// A file on the device's filesystem.
    File(PathBuf),
    /// An asset shipped inside the app bundle, by name: a path under
    /// Android's `assets/` directory, or a main-bundle resource path on
    /// Apple. Not a filesystem path — the platform resolves it.
    Asset(String),
    /// A remote URL: `http(s)` progressive download, or an HLS playlist
    /// (`.m3u8`) — see the crate doc on why HLS support is per-platform.
    Url(String),
}

/// How the picture fills its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoFit {
    /// Fit the whole picture inside the slot, letterboxing the remainder
    /// (`AVLayerVideoGravityResizeAspect`). The default: nothing is cropped.
    #[default]
    Contain,
    /// Fill the slot completely, cropping whichever axis overflows
    /// (`AVLayerVideoGravityResizeAspectFill`).
    Cover,
}

impl VideoFit {
    /// This fit's spelling in a params payload — frozen, since the native
    /// factories parse it.
    fn as_str(self) -> &'static str {
        match self {
            Self::Contain => "contain",
            Self::Cover => "cover",
        }
    }
}

/// How a session starts out, fixed at [`VideoPlayer::open`].
///
/// Everything here except [`Self::mix_with_others`] is also settable later
/// through [`PlayerSession`]; these are the values the platform player is
/// configured with before the first frame, so an autoplaying session never
/// flashes a paused frame first.
#[derive(Clone, Debug)]
pub struct PlayerOptions {
    /// Start playing as soon as the item is ready, rather than waiting for
    /// [`PlayerSession::play`]. Default `false`.
    pub autoplay: bool,
    /// Restart from the beginning at the end of the item instead of
    /// reporting [`PlaybackState::Ended`]. Default `false`.
    pub looping: bool,
    /// Output volume, `0.0..=1.0`; values outside that range are clamped by
    /// the platform. Default `1.0`.
    pub volume: f32,
    /// iOS only: configure the app's `AVAudioSession` with
    /// `.mixWithOthers`, so playback ducks alongside other apps' audio
    /// instead of interrupting it. Ignored on every other platform — a
    /// capability gap is recorded per-platform, never emulated onto the
    /// platform that doesn't have it. Default `false`.
    pub mix_with_others: bool,
}

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            autoplay: false,
            looping: false,
            volume: 1.0,
            mix_with_others: false,
        }
    }
}

/// Where a session is in its lifecycle.
///
/// The numbering is a **frozen contract**: each platform host reports its
/// own state as one of these codes over its FFI surface, and
/// [`PlaybackState::from_code`] is the only place they are decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    /// Nothing loaded yet — a session's state before its backend publishes.
    Idle,
    /// The item is being prepared; no frame is available yet.
    Loading,
    /// Ready, and not advancing.
    Paused,
    /// Advancing.
    Playing,
    /// Playing was requested, but the player is refilling its buffer.
    Buffering,
    /// Played to the end (never reported while looping — the item restarts
    /// instead).
    Ended,
    /// Failed; [`PlayerSnapshot::error`] carries the reason.
    Error,
}

impl PlaybackState {
    /// Decode a host-reported state code, or `None` if the host sent a value
    /// outside the frozen `0..=6` contract (a host newer than this crate).
    /// Never a panic on an FFI path.
    pub(crate) fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Idle),
            1 => Some(Self::Loading),
            2 => Some(Self::Paused),
            3 => Some(Self::Playing),
            4 => Some(Self::Buffering),
            5 => Some(Self::Ended),
            6 => Some(Self::Error),
            _ => None,
        }
    }

    /// This state's wire code — the inverse of [`Self::from_code`], and how
    /// the snapshot stores it in a single atomic.
    // Written only by the publishing half, which has no caller in a non-test
    // library build — see `snapshot`'s module doc for the whole rationale.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Loading => 1,
            Self::Paused => 2,
            Self::Playing => 3,
            Self::Buffering => 4,
            Self::Ended => 5,
            Self::Error => 6,
        }
    }
}

/// Everything a frame build needs to know about a session, read atomically
/// and without blocking — see [`PlayerSession::snapshot`].
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
    /// The lifecycle state.
    pub state: PlaybackState,
    /// The current playback position.
    pub position: Duration,
    /// The item's total duration, or `None` while it is still unknown —
    /// before the item is ready, and forever for a live stream.
    pub duration: Option<Duration>,
    /// The decoded picture's pixel size, or `None` before the platform has
    /// reported the first frame's geometry. Size a slot from this rather
    /// than assuming an aspect.
    pub video_size: Option<(u32, u32)>,
    /// The failure behind [`PlaybackState::Error`]; `None` in every other
    /// state.
    pub error: Option<VideoError>,
}

/// One thing that happened to a session, delivered to its listener on the
/// platform main thread after the snapshot has already been updated.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    /// The lifecycle state changed.
    StateChanged(PlaybackState),
    /// The position advanced (reported periodically while playing, and once
    /// after a seek completes), carrying the duration as currently known.
    Position {
        /// The new playback position.
        position: Duration,
        /// The item's total duration, if known yet.
        duration: Option<Duration>,
    },
    /// The decoded picture's pixel size became known, or changed.
    VideoSize {
        /// Decoded width in pixels.
        width: u32,
        /// Decoded height in pixels.
        height: u32,
    },
    /// Playback failed. Publishing this also moves the session to
    /// [`PlaybackState::Error`] and fills [`PlayerSnapshot::error`], so a
    /// listener and a snapshot reader agree about a failure.
    Error(VideoError),
}

/// A video-playback failure.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant (retry a [`Self::Network`], report a [`Self::Decoder`], hide the
/// control entirely on [`Self::NotSupported`]) rather than only displaying
/// it.
#[derive(thiserror::Error, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum VideoError {
    /// This target has no video backend at all (crate doc's *Backends*) —
    /// desktop Linux/Windows, wasm. Fail soft, never a panic.
    #[error("video playback is not supported on this platform")]
    NotSupported,
    /// The host shell never installed the platform handles this backend
    /// needs (Android: the `(JavaVM, Context)` pair `frust-plugin`
    /// publishes), or the native player class could not be resolved. An app
    /// scaffolded before the plugin's platform module was wired in.
    #[error("the video platform is not initialized")]
    PlatformNotInitialized,
    /// The host reported an answer for a session this side no longer knows
    /// — a late callback for a closed session, in practice.
    #[error("unknown video session")]
    UnknownSession,
    /// The platform refused the source itself: an unreadable path, an asset
    /// name that resolves to nothing, a URL it will not accept.
    #[error("unsupported video source: {0}")]
    UnsupportedSource(String),
    /// A transport failure reaching a remote source — retryable, unlike
    /// [`Self::Decoder`].
    #[error("video network error: {0}")]
    Network(String),
    /// The platform could not decode the media: an unsupported container or
    /// codec on *this* platform (crate doc's format-asymmetry note), or
    /// corrupt data.
    #[error("video decode error: {0}")]
    Decoder(String),
    /// A control call was made from inside a listener invocation, which
    /// would re-enter the backend mid-event (crate doc's *Threading*). Move
    /// the call out of the callback.
    #[error("video control call re-entered from inside an event listener")]
    Reentrant,
    /// The session has been closed; every control call reports this instead
    /// of reaching a released platform player.
    #[error("the video session is closed")]
    Closed,
    /// Anything the platform reported that no variant above names.
    #[error("video platform error: {0}")]
    Host(String),
}

impl VideoError {
    /// Decode a host-reported error code plus its message — the frozen
    /// contract shared by every backend's FFI surface. An unrecognized code
    /// becomes [`Self::Host`] rather than a panic, so a host newer than this
    /// crate degrades to a readable message.
    // Called from each backend's host-callback path, none of which exists
    // yet, and from the test suite that pins the table.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_host_code(code: i32, message: &str) -> Self {
        match code {
            -1 => Self::UnknownSession,
            -2 => Self::UnsupportedSource(message.to_owned()),
            -3 => Self::PlatformNotInitialized,
            -4 => Self::Network(message.to_owned()),
            -5 => Self::Decoder(message.to_owned()),
            _ => Self::Host(message.to_owned()),
        }
    }
}

/// The crate's one entry point: opens sessions.
pub struct VideoPlayer;

impl VideoPlayer {
    /// Start loading `source` and return its session **immediately**.
    ///
    /// Non-blocking and callable from any thread, the UI thread included:
    /// this hands the source to the platform player and returns without
    /// waiting for it to load, so an event handler can open a video without
    /// stalling a frame. The session starts in [`PlaybackState::Loading`];
    /// readiness, the first frame's geometry, and any load failure arrive
    /// afterwards through [`PlayerSession::snapshot`] and the session's
    /// listener.
    ///
    /// # Errors
    /// [`VideoError::NotSupported`] on a target with no video backend;
    /// [`VideoError::PlatformNotInitialized`] when the host shell never
    /// installed this platform's handles; [`VideoError::UnsupportedSource`]
    /// when the platform refuses the source outright. A failure to *load*
    /// is not reported here — see the note above.
    pub fn open(source: VideoSource, options: PlayerOptions) -> Result<PlayerSession, VideoError> {
        let shared = Arc::new(snapshot::Shared::new());
        let session =
            <backend::Active as PlayerBackend>::open(&source, &options, Arc::clone(&shared))?;

        Ok(PlayerSession { session, shared })
    }
}

/// An open video session: the controls, the state, and the parameters its
/// native view attaches through.
///
/// `Send + Sync` — hold one in app state and drive it from anywhere. Every
/// method below is non-blocking (crate doc's *Threading*); each control
/// method posts a command at the platform player and reports only whether
/// the command was *accepted*, never its outcome.
pub struct PlayerSession {
    session: backend::ActiveSession,
    shared: Arc<snapshot::Shared>,
}

impl PlayerSession {
    /// Start (or resume) playback.
    ///
    /// # Errors
    /// [`VideoError::Reentrant`] from inside a listener,
    /// [`VideoError::Closed`] after [`Self::close`], or whatever the
    /// platform refuses.
    pub fn play(&self) -> Result<(), VideoError> {
        self.guard()?;
        self.session.play()
    }

    /// Pause playback, keeping the position.
    ///
    /// # Errors
    /// As [`Self::play`].
    pub fn pause(&self) -> Result<(), VideoError> {
        self.guard()?;
        self.session.pause()
    }

    /// Seek to `position`. The platform clamps it to the item's range, and
    /// publishes a [`PlayerEvent::Position`] when the seek lands.
    ///
    /// # Errors
    /// As [`Self::play`].
    pub fn seek_to(&self, position: Duration) -> Result<(), VideoError> {
        self.guard()?;
        self.session.seek_to(position)
    }

    /// Set the playback rate — `1.0` is normal speed. A non-zero rate on a
    /// paused player starts it, matching the platform players' own
    /// behaviour.
    ///
    /// # Errors
    /// As [`Self::play`].
    pub fn set_rate(&self, rate: f32) -> Result<(), VideoError> {
        self.guard()?;
        self.session.set_rate(rate)
    }

    /// Set the output volume, `0.0..=1.0` (clamped by the platform).
    ///
    /// # Errors
    /// As [`Self::play`].
    pub fn set_volume(&self, volume: f32) -> Result<(), VideoError> {
        self.guard()?;
        self.session.set_volume(volume)
    }

    /// Turn looping on or off, overriding [`PlayerOptions::looping`].
    ///
    /// # Errors
    /// As [`Self::play`].
    pub fn set_looping(&self, looping: bool) -> Result<(), VideoError> {
        self.guard()?;
        self.session.set_looping(looping)
    }

    /// Release the platform player. **Idempotent**: the second and every
    /// later call does nothing, and dropping a session runs exactly this,
    /// so an app that never calls it still releases the player.
    ///
    /// Every control call after this reports [`VideoError::Closed`];
    /// [`Self::snapshot`] keeps answering with the last published state.
    ///
    /// Unlike the control methods this is *not* refused from inside a
    /// listener: it returns nothing, so it could not report a refusal, and
    /// closing a failed session from its own error callback is the obvious
    /// thing to want. Backends must therefore tolerate a `close` issued
    /// from inside a callback — the one call that has to.
    pub fn close(&self) {
        if self.shared.close() {
            self.session.close();
        }
    }

    /// The session's current state, read without blocking from any thread —
    /// what a frame build calls.
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.shared.snapshot()
    }

    /// Register `callback` as this session's listener, replacing any
    /// previous one, and return the handle that keeps it registered.
    ///
    /// **One listener per session.** A second call replaces the first, and
    /// the first call's handle becomes inert — dropping it afterwards
    /// removes nothing, so a stale handle can never silently unregister a
    /// newer listener.
    ///
    /// The callback runs on the **platform main thread**, after the
    /// snapshot has been updated with the same event. Two rules bind it: it
    /// must never block (it is holding the thread the whole UI runs on), and
    /// it must never call back into this session — a control call from
    /// inside it is refused with [`VideoError::Reentrant`] instead of
    /// re-entering the backend. Write a signal and return.
    pub fn set_listener(&self, callback: Box<dyn Fn(PlayerEvent) + Send + Sync>) -> ListenerHandle {
        let generation = self.shared.set_listener(callback);
        ListenerHandle {
            shared: Arc::downgrade(&self.shared),
            generation,
        }
    }

    /// The `viewType` to pass to `frust::platform_view` for this session's
    /// picture — the [`VIEW_TYPE`] constant, target-gated (crate doc).
    pub fn view_type(&self) -> &'static str {
        VIEW_TYPE
    }

    /// This session's platform-view parameters as JSON: which session the
    /// native view should attach to, and how the picture fills the slot.
    ///
    /// The payload an app threads into `platform_view(...).params_json(...)`,
    /// so changing the fit reaches the native factory as a params update
    /// rather than a slot teardown. Both the key spelling (`sessionId` on
    /// Android, `session` on Apple) and the fit spellings are frozen — the
    /// native factories parse them.
    pub fn params_json(&self, fit: VideoFit) -> String {
        params_json_with(SESSION_KEY, self.session.id(), fit)
    }

    /// The guard every control method runs first: refuse a re-entrant call,
    /// then a call on a closed session.
    fn guard(&self) -> Result<(), VideoError> {
        snapshot::reject_if_delivering()?;
        if self.shared.is_closed() {
            return Err(VideoError::Closed);
        }
        Ok(())
    }

    /// The backend session behind this handle, for the test suite's own
    /// rigging — see [`mock::MockSession::host`].
    #[cfg(test)]
    pub(crate) fn mock_host(&self) -> mock::MockHost {
        self.session.host()
    }
}

impl Drop for PlayerSession {
    fn drop(&mut self) {
        // Identical to an explicit `close`, and idempotent with one: an app
        // that dropped the session still releases the platform player.
        self.close();
    }
}

/// Build a params payload with an explicit key spelling — the seam that lets
/// the host-side suite assert both platforms' spellings wherever it runs,
/// while [`PlayerSession::params_json`] only ever passes the target's own.
fn params_json_with(session_key: &str, session_id: u32, fit: VideoFit) -> String {
    format!(
        "{{\"{session_key}\":{session_id},\"fit\":\"{fit}\"}}",
        fit = fit.as_str()
    )
}

/// A registered [`PlayerEvent`] listener. **Dropping this unregisters it** —
/// see [`PlayerSession::set_listener`], the only way to obtain one.
///
/// Deliberately opaque and not `Clone`: exactly one handle owns each
/// registration. A handle whose registration was already replaced by a later
/// `set_listener` is inert, and a handle outliving its session removes
/// nothing (it holds a weak reference, so it never keeps a closed session's
/// state alive).
#[derive(Debug)]
pub struct ListenerHandle {
    shared: Weak<snapshot::Shared>,
    generation: u64,
}

impl ListenerHandle {
    /// Unregister the listener now — identical in effect to dropping the
    /// handle, and worth spelling out where the drop would otherwise be
    /// invisible. No event published after this returns reaches the
    /// callback.
    pub fn remove(self) {
        // `Drop` does the unregistration; consuming `self` is what makes the
        // intent readable, and guarantees the handle can't be used after.
        drop(self);
    }
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.remove_listener(self.generation);
        }
    }
}
