//! A `#[cfg(test)]` in-memory player implementing the [`PlayerBackend`] seam
//! — the fixture [`crate::conformance`] runs against on every host.
//!
//! No real backend can run host-side (`android`/`apple` compile only for
//! their own targets, and even there they need a real display and real
//! media), so without this the crate's whole platform-independent contract —
//! the snapshot, the listener registry, the re-entrancy refusal, the close
//! semantics — would be checked by nothing but a device gate. This player
//! sits behind the *same* trait pair the real backends implement, and
//! [`crate::backend::Active`] resolves to it under `cfg(test)` on every
//! target, so the suite exercises the real dispatch path rather than a
//! parallel one.
//!
//! # What it fakes, and what it deliberately does not
//!
//! It records every command it is given, in order, and publishes exactly the
//! events a test tells it to through [`MockHost`]. It does **not** advance a
//! clock, decode anything, or invent state transitions of its own: a real
//! backend's control call is non-blocking and reports nothing about its
//! outcome (the platform publishes that later, from its own thread), so a
//! mock that flipped the state from inside `play` would be testing a
//! contract this crate does not have. [`MockHost`] therefore stands in for
//! the platform callback, emitting on the calling thread exactly as a
//! platform callback does on the main one.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use frust_plugin::texture::{TextureFrame, TextureSink};

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlaybackState, PlayerEvent, PlayerOptions, VideoError, VideoOutput, VideoSource};

/// One command the public API dispatched into the backend.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Play,
    Pause,
    SeekTo(Duration),
    SetRate(f32),
    SetVolume(f32),
    SetLooping(bool),
    Close,
}

/// Session ids, assigned the way a real host assigns them: increasing, never
/// reused, and shared across the whole process.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// The state one mock session and its [`MockHost`] share.
struct MockState {
    id: u32,
    shared: Arc<Shared>,
    source: VideoSource,
    options: PlayerOptions,
    commands: Mutex<Vec<Command>>,
    /// The refusal the next control call answers with, armed by
    /// [`MockHost::refuse_next`] — how a test drives the
    /// platform-refused-the-command path.
    refusal: Mutex<Option<VideoError>>,
    /// The frame mailbox, created at open for a [`VideoOutput::Texture`]
    /// session — the producer half a real backend's decoder pushes into.
    sink: Option<TextureSink>,
    /// How many frames [`MockHost::emit_frame`] pushed have been released.
    released: Arc<AtomicUsize>,
}

/// Opens mock sessions.
pub(crate) struct MockBackend;

impl PlayerBackend for MockBackend {
    type Session = MockSession;

    fn open(
        source: &VideoSource,
        options: &PlayerOptions,
        shared: Arc<Shared>,
    ) -> Result<Self::Session, VideoError> {
        // The one thing a real backend can refuse synchronously: a source it
        // cannot even form a request from.
        if source_is_empty(source) {
            return Err(VideoError::UnsupportedSource("empty source".to_owned()));
        }

        // A real backend starts loading before it returns, so the session is
        // never observed in `Idle`.
        shared.publish(PlayerEvent::StateChanged(PlaybackState::Loading));

        Ok(MockSession {
            state: Arc::new(MockState {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                shared,
                source: source.clone(),
                options: options.clone(),
                commands: Mutex::new(Vec::new()),
                refusal: Mutex::new(None),
                sink: (options.output == VideoOutput::Texture).then(TextureSink::new),
                released: Arc::new(AtomicUsize::new(0)),
            }),
        })
    }
}

/// Whether `source` names nothing at all.
fn source_is_empty(source: &VideoSource) -> bool {
    match source {
        VideoSource::File(path) => path.as_os_str().is_empty(),
        VideoSource::Asset(name) => name.is_empty(),
        VideoSource::Url(url) => url.is_empty(),
    }
}

/// One open mock session.
pub(crate) struct MockSession {
    state: Arc<MockState>,
}

impl MockSession {
    /// The test-side handle onto this session — the platform's half of the
    /// contract (see [`MockHost`]).
    pub(crate) fn host(&self) -> MockHost {
        MockHost {
            state: Arc::clone(&self.state),
        }
    }

    /// Record `command`, then answer with an armed refusal if there is one.
    fn record(&self, command: Command) -> Result<(), VideoError> {
        lock(&self.state.commands).push(command);
        match lock(&self.state.refusal).take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl BackendSession for MockSession {
    fn play(&self) -> Result<(), VideoError> {
        self.record(Command::Play)
    }

    fn pause(&self) -> Result<(), VideoError> {
        self.record(Command::Pause)
    }

    fn seek_to(&self, position: Duration) -> Result<(), VideoError> {
        self.record(Command::SeekTo(position))
    }

    fn set_rate(&self, rate: f32) -> Result<(), VideoError> {
        self.record(Command::SetRate(rate))
    }

    fn set_volume(&self, volume: f32) -> Result<(), VideoError> {
        self.record(Command::SetVolume(volume))
    }

    fn set_looping(&self, looping: bool) -> Result<(), VideoError> {
        self.record(Command::SetLooping(looping))
    }

    /// Record the close, then publish `Idle` — matching the post-close
    /// contract every backend answers (`android.rs`'s `AndroidSession::close`,
    /// `apple.rs`'s `teardown`). Called at most once per session
    /// ([`crate::backend::BackendSession::close`]'s own doc), so this needs
    /// no idempotence of its own.
    fn close(&self) {
        lock(&self.state.commands).push(Command::Close);
        // A real backend stops its producer here; the frame still pending in
        // the mailbox will never be shown, so it is handed back now rather
        // than when the last sink clone happens to drop.
        if let Some(frame) = self.state.sink.as_ref().and_then(TextureSink::take_latest) {
            frame.release();
        }
        self.state
            .shared
            .publish(PlayerEvent::StateChanged(PlaybackState::Idle));
    }

    fn id(&self) -> u32 {
        self.state.id
    }

    fn texture_sink(&self) -> Option<&TextureSink> {
        self.state.sink.as_ref()
    }
}

/// The platform's half of a mock session: what a test emits through, and
/// what it reads the recorded commands back from.
///
/// Obtained from the session itself
/// ([`crate::PlayerSession::mock_host`]) rather than from a process-global
/// registry, so tests running in parallel never see each other's sessions.
pub(crate) struct MockHost {
    state: Arc<MockState>,
}

impl MockHost {
    /// Publish a state change, as a platform callback would.
    pub(crate) fn emit_state(&self, state: PlaybackState) {
        self.state.shared.publish(PlayerEvent::StateChanged(state));
    }

    /// Publish a position tick, with the duration as currently known.
    pub(crate) fn emit_position(&self, position: Duration, duration: Option<Duration>) {
        self.state
            .shared
            .publish(PlayerEvent::Position { position, duration });
    }

    /// Publish the decoded picture's geometry.
    pub(crate) fn emit_video_size(&self, width: u32, height: u32) {
        self.state
            .shared
            .publish(PlayerEvent::VideoSize { width, height });
    }

    /// Publish a failure, using the same host-code decoding every real
    /// backend's FFI surface uses.
    pub(crate) fn emit_error(&self, code: i32, message: &str) {
        self.state
            .shared
            .publish(PlayerEvent::Error(VideoError::from_host_code(
                code, message,
            )));
    }

    /// Push one synthetic RGBA frame into the session's mailbox, as a real
    /// backend's frame producer would; its release bumps
    /// [`Self::frames_released`]. A no-op for a session with no mailbox (one
    /// opened with [`VideoOutput::NativeView`]).
    pub(crate) fn emit_frame(&self, width: u32, height: u32, rgba: &[u8]) {
        let Some(sink) = self.state.sink.as_ref() else {
            return;
        };
        let released = Arc::clone(&self.state.released);
        sink.push(
            TextureFrame::rgba8(width, height, Arc::from(rgba)).on_release(move || {
                released.fetch_add(1, Ordering::SeqCst);
            }),
        );
    }

    /// How many frames pushed by [`Self::emit_frame`] have been released so
    /// far — displaced by a newer one, consumed and released, or released by
    /// the close.
    pub(crate) fn frames_released(&self) -> usize {
        self.state.released.load(Ordering::SeqCst)
    }

    /// Arm the next control call to be refused with `error`.
    pub(crate) fn refuse_next(&self, error: VideoError) {
        *lock(&self.state.refusal) = Some(error);
    }

    /// Every command this session has been given, in order.
    pub(crate) fn commands(&self) -> Vec<Command> {
        lock(&self.state.commands).clone()
    }

    /// The source the session was opened with.
    pub(crate) fn source(&self) -> VideoSource {
        self.state.source.clone()
    }

    /// The options the session was opened with.
    pub(crate) fn options(&self) -> PlayerOptions {
        self.state.options.clone()
    }

    /// This session's id — what [`crate::PlayerSession::params_json`]
    /// publishes.
    pub(crate) fn id(&self) -> u32 {
        self.state.id
    }
}

/// Lock `mutex`, recovering from poisoning rather than propagating it, so one
/// failing assertion inside a listener doesn't cascade into unrelated
/// failures.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
