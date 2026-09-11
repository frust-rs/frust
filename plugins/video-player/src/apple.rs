//! Placeholder AVFoundation backend: the module exists, is wired into
//! [`crate::backend::Active`], and compiles for iOS **and** macOS (it is
//! gated `target_vendor = "apple"`, so a native macOS host build compiles it
//! too), but holds no `AVPlayer` yet — the real implementation replaces this
//! file wholesale.
//!
//! Until then [`AppleBackend::open`] refuses with
//! [`VideoError::NotSupported`], the same fail-soft shape
//! [`crate::unsupported`] uses, so an Apple build of an app written against
//! this crate links and degrades rather than panicking.
//!
//! # Why there is no C export here
//!
//! The native video views on both Apple platforms are written in Rust
//! ([`crate::ios_view`], [`crate::macos_view`]) and live in this same crate,
//! so they reach a session's player through [`player_for`] — a crate-private
//! accessor — rather than through an `extern "C"` symbol an app would have
//! to keep alive against the linker. Nothing in this crate is exported over
//! the C ABI.

// Every item here is referenced only through `crate::backend::Active` and
// the (not yet written) view factories, and `open`'s refusal means the
// session type is never constructed — which is exactly what a placeholder
// looks like to the dead-code lint.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlayerOptions, VideoError, VideoSource};

/// Opens AVFoundation player sessions, on iOS and macOS alike.
pub(crate) struct AppleBackend;

impl PlayerBackend for AppleBackend {
    type Session = AppleSession;

    /// # Errors
    /// Always [`VideoError::NotSupported`] while this backend is a
    /// placeholder (module doc).
    fn open(
        _source: &VideoSource,
        _options: &PlayerOptions,
        _shared: Arc<Shared>,
    ) -> Result<Self::Session, VideoError> {
        Err(VideoError::NotSupported)
    }
}

/// One AVFoundation player session.
pub(crate) struct AppleSession {
    /// The id the native view's params carry, and the key [`player_for`]
    /// looks a session up by.
    id: u32,
}

impl BackendSession for AppleSession {
    fn play(&self) -> Result<(), VideoError> {
        Err(VideoError::NotSupported)
    }

    fn pause(&self) -> Result<(), VideoError> {
        Err(VideoError::NotSupported)
    }

    fn seek_to(&self, _position: Duration) -> Result<(), VideoError> {
        Err(VideoError::NotSupported)
    }

    fn set_rate(&self, _rate: f32) -> Result<(), VideoError> {
        Err(VideoError::NotSupported)
    }

    fn set_volume(&self, _volume: f32) -> Result<(), VideoError> {
        Err(VideoError::NotSupported)
    }

    fn set_looping(&self, _looping: bool) -> Result<(), VideoError> {
        Err(VideoError::NotSupported)
    }

    fn close(&self) {}

    fn id(&self) -> u32 {
        self.id
    }
}

/// Look up the live player behind a session id — how the Rust view
/// factories on both Apple platforms attach a layer to a session without any
/// FFI export (module doc).
///
/// The id is signed because it arrives from the native side as the params
/// payload's session number, and a hostile or stale payload may carry
/// anything; an unknown id answers `None` rather than failing.
///
/// Answers `None` for every id while this backend is a placeholder. The real
/// backend keeps this signature but returns the retained `AVPlayer` the
/// factory attaches (`Option<Retained<AVPlayer>>`) — the change is the
/// return type only, so call sites written against `is_some()` semantics
/// keep working.
pub(crate) fn player_for(session: i32) -> Option<()> {
    let _ = session;
    None
}
