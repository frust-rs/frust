//! Placeholder Android backend: the module exists, is wired into
//! [`crate::backend::Active`], and compiles for `aarch64-linux-android`, but
//! holds no player yet — the real ExoPlayer-side implementation replaces
//! this file wholesale.
//!
//! Until then [`AndroidBackend::open`] refuses with
//! [`VideoError::NotSupported`], the same fail-soft shape
//! [`crate::unsupported`] uses, so an Android build of an app written
//! against this crate links and degrades rather than panicking.

// Every item here is referenced only through `crate::backend::Active`, and
// `open`'s refusal means the session type is never constructed — which is
// exactly what a placeholder looks like to the dead-code lint.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlayerOptions, VideoError, VideoSource};

/// Opens Android player sessions.
pub(crate) struct AndroidBackend;

impl PlayerBackend for AndroidBackend {
    type Session = AndroidSession;

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

/// One Android player session.
pub(crate) struct AndroidSession {
    /// The id the Kotlin side knows this session by, and the value
    /// [`crate::PlayerSession::params_json`] publishes to the native view.
    id: u32,
}

impl BackendSession for AndroidSession {
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
