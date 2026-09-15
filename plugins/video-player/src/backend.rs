//! The crate-private seam every platform arm implements, and the one type
//! alias that picks the arm for the target being compiled.
//!
//! [`crate::PlayerSession`] is deliberately **not** generic and holds no
//! `dyn` object: exactly one backend exists per target, so [`Active`] names
//! it directly and the public API carries no type parameter an app would
//! have to spell. The seam still exists as a trait pair because the test
//! suite drives a fake through it — see [`Active`]'s own doc for why the
//! test alias, not the module gating, is what decides that.
//!
//! Every method here is **non-blocking**: a backend hands the command to the
//! platform's player and returns. Outcomes come back asynchronously through
//! [`crate::snapshot::Shared::publish`], never as a return value.

use std::sync::Arc;
use std::time::Duration;

use crate::snapshot::Shared;
use crate::{PlayerOptions, VideoError, VideoSource};

/// Opens sessions on one platform's player.
pub(crate) trait PlayerBackend {
    /// The session type this backend's [`Self::open`] produces.
    type Session: BackendSession;

    /// Start loading `source` and return its session immediately.
    ///
    /// `shared` is where the session publishes every later state, position,
    /// size, and error; the backend keeps its own clone for the lifetime of
    /// the platform player.
    ///
    /// # Errors
    /// Whatever the platform refuses at construction time — an unusable
    /// source, a missing platform handle, or
    /// [`VideoError::NotSupported`] on a target with no backend at all.
    /// A failure to *load* is not reported here: loading is asynchronous, so
    /// it arrives as a published [`crate::PlayerEvent::Error`].
    fn open(
        source: &VideoSource,
        options: &PlayerOptions,
        shared: Arc<Shared>,
    ) -> Result<Self::Session, VideoError>;
}

/// One open player, as the public [`crate::PlayerSession`] drives it.
///
/// `Send + Sync` because [`crate::PlayerSession`] is: an app holds one in
/// shared state and issues commands from whatever thread it is on.
pub(crate) trait BackendSession: Send + Sync {
    /// Start (or resume) playback.
    fn play(&self) -> Result<(), VideoError>;
    /// Pause playback, keeping the position.
    fn pause(&self) -> Result<(), VideoError>;
    /// Seek to `position`, clamped by the platform to the item's range.
    fn seek_to(&self, position: Duration) -> Result<(), VideoError>;
    /// Set the playback rate (`1.0` = normal speed).
    fn set_rate(&self, rate: f32) -> Result<(), VideoError>;
    /// Set the output volume, `0.0..=1.0`.
    fn set_volume(&self, volume: f32) -> Result<(), VideoError>;
    /// Turn looping on or off.
    fn set_looping(&self, looping: bool) -> Result<(), VideoError>;
    /// Release the platform player. Called at most once per session — the
    /// public wrapper's close flag is what guarantees that, so a backend
    /// needs no idempotence of its own.
    fn close(&self);
    /// This session's id, the value the platform view's params carry so the
    /// native factory can find the player to attach (see
    /// [`crate::PlayerSession::params_json`]).
    fn id(&self) -> u32;
}

/// The backend compiled into this build.
///
/// Under `cfg(test)` this is the in-memory [`crate::mock`] backend **on
/// every host**, including an Apple one where [`crate::apple`] itself
/// compiles: the module gating decides what *exists*, this alias decides
/// what the suite drives, and the suite must drive the fake rather than a
/// real AVPlayer.
#[cfg(test)]
pub(crate) type Active = crate::mock::MockBackend;

/// The Android player backend.
#[cfg(all(not(test), target_os = "android"))]
pub(crate) type Active = crate::android::AndroidBackend;

/// The AVFoundation backend, shared by iOS and macOS.
#[cfg(all(not(test), target_vendor = "apple"))]
pub(crate) type Active = crate::apple::AppleBackend;

/// The inert backend for a target with no player at all.
#[cfg(all(not(test), not(any(target_os = "android", target_vendor = "apple"))))]
pub(crate) type Active = crate::unsupported::UnsupportedBackend;

/// The session type [`Active`] produces — what [`crate::PlayerSession`]
/// stores.
pub(crate) type ActiveSession = <Active as PlayerBackend>::Session;
