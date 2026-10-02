//! The inert backend for every target that is neither Android nor Apple
//! (desktop Linux/Windows, wasm, anything else this crate has no player
//! for).
//!
//! Mirrors `frust-camera`'s `unsupported` arm: the one entry point fails
//! soft with [`VideoError::NotSupported`], never a panic, and never
//! constructs a session — there is no platform player for one to wrap. That
//! is also why [`UnsupportedSession`] is an empty enum: the refusal happens
//! before a session value could exist, so the type is uninhabited by
//! construction and its [`BackendSession`] methods are unreachable rather
//! than stubbed with panics.

use std::sync::Arc;
use std::time::Duration;

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlayerOptions, VideoError, VideoSource};

/// [`crate::VideoPlayer::open`]'s no-backend arm.
pub(crate) struct UnsupportedBackend;

impl PlayerBackend for UnsupportedBackend {
    type Session = UnsupportedSession;

    /// Always refuses — see the module doc.
    ///
    /// # Errors
    /// Always [`VideoError::NotSupported`].
    fn open(
        _source: &VideoSource,
        _options: &PlayerOptions,
        _shared: Arc<Shared>,
    ) -> Result<Self::Session, VideoError> {
        Err(VideoError::NotSupported)
    }
}

/// A session on a target with no player — deliberately uninhabited, since
/// [`UnsupportedBackend::open`] never produces one (module doc).
pub(crate) enum UnsupportedSession {}

impl BackendSession for UnsupportedSession {
    fn play(&self) -> Result<(), VideoError> {
        match *self {}
    }

    fn pause(&self) -> Result<(), VideoError> {
        match *self {}
    }

    fn seek_to(&self, _position: Duration) -> Result<(), VideoError> {
        match *self {}
    }

    fn set_rate(&self, _rate: f32) -> Result<(), VideoError> {
        match *self {}
    }

    fn set_volume(&self, _volume: f32) -> Result<(), VideoError> {
        match *self {}
    }

    fn set_looping(&self, _looping: bool) -> Result<(), VideoError> {
        match *self {}
    }

    fn close(&self) {
        match *self {}
    }

    fn id(&self) -> u32 {
        match *self {}
    }
}
