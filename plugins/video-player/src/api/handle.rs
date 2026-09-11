//! [`VideoPlayerHandle`]: the reactive glue between a [`crate::PlayerSession`]
//! and a `frust` component — five `RwSignal`s a `Component::build` reads,
//! kept in sync by exactly one [`crate::PlayerSession::set_listener`]
//! registration.
//!
//! # Threading: the listener runs on the platform main thread
//!
//! [`VideoPlayerHandle::new`] registers a listener closure with the session
//! (see the crate's own doc's *Threading* section): every backend delivers
//! [`PlayerEvent`]s from the platform's own main thread, which is frust's
//! thread on every supported shell (desktop's winit loop included on
//! macOS), so a plain [`RwSignal::set`] inside that closure is correct and
//! needs no cross-thread hop. Any number of writes between two rebuilds
//! coalesce into one wake (`docs/ARCHITECTURE.md`'s Signal-driven wake), and
//! per `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions the
//! closure only ever **writes** — reading a signal happens from a
//! component's `build`, never from in here. The closure must never block
//! and must never call back into the session it was registered on: a
//! control call made from inside a listener delivery is refused with
//! [`VideoError::Reentrant`] rather than re-entering the backend
//! mid-callback (see [`crate::PlayerSession::set_listener`]).

use std::sync::Arc;
use std::time::Duration;

use frust::{RwSignal, Set, on_cleanup};

use crate::{
    ListenerHandle, PlaybackState, PlayerEvent, PlayerSession, PlayerSnapshot, VideoError,
};

/// A [`crate::PlayerSession`] paired with five tracked signals a
/// [`frust::Component::build`] reads, kept current by the session's one
/// listener registration.
///
/// # One handle per session
///
/// A session accepts exactly one listener at a time
/// ([`crate::PlayerSession::set_listener`]): constructing a second
/// [`VideoPlayerHandle`] over the same session replaces the first
/// registration, and the first handle's signals silently stop updating even
/// though the handle itself is still alive. [`VideoPlayerHandle`]
/// deliberately does **not** implement `Clone`, for the identical reason
/// [`crate::ListenerHandle`] does not: cloning it would let two call sites
/// believe they each independently own the listener registration, when only
/// one registration — the most recently installed — actually exists.
///
/// # Dropping this handle
///
/// A handle obtained from [`Self::new`] holds the *only* reference to its
/// [`crate::ListenerHandle`], so dropping it (or letting it go out of scope)
/// unregisters the listener immediately — the five signals simply stop
/// receiving updates and keep reporting whatever they last saw. The handle
/// also owns the [`crate::PlayerSession`] it was built from, so dropping the
/// last handle drops that session too, and the session's own `Drop` closes
/// the platform player. The listener is always unregistered *before* the
/// session goes (field order below is load-bearing), so the `Idle` a
/// backend publishes as its close completes never reaches the signals.
/// Call [`Self::close`] explicitly to release the player while the handle
/// is still alive. A handle obtained from [`use_video_player`] shares that
/// reference with a registered `on_cleanup`, so an *early* drop of the
/// returned handle alone does not unregister the listener — see that
/// function's own doc.
pub struct VideoPlayerHandle {
    // Declared first so it drops first: Rust drops fields in declaration
    // order, and the listener must be unregistered before `session` — the
    // last strong reference in ordinary use — closes the backend and
    // publishes its final `Idle`.
    listener: Arc<ListenerHandle>,
    session: Arc<PlayerSession>,
    /// The session's lifecycle state, seeded from [`PlayerSnapshot::state`]
    /// and updated on every [`PlayerEvent::StateChanged`] — also written to
    /// [`PlaybackState::Error`] on a [`PlayerEvent::Error`], mirroring
    /// [`crate::PlayerSession::snapshot`]'s own state/error coupling so a
    /// `build` reading both signals never observes them disagree.
    pub state: RwSignal<PlaybackState>,
    /// The current playback position, updated on every
    /// [`PlayerEvent::Position`].
    pub position: RwSignal<Duration>,
    /// The item's total duration, or `None` while unknown — updated
    /// alongside [`Self::position`] on every [`PlayerEvent::Position`] (the
    /// event always carries both).
    pub duration: RwSignal<Option<Duration>>,
    /// The decoded picture's pixel size, or `None` before the platform has
    /// reported it — updated on [`PlayerEvent::VideoSize`].
    pub video_size: RwSignal<Option<(u32, u32)>>,
    /// The failure behind [`PlaybackState::Error`], or `None` in every
    /// other state — updated on [`PlayerEvent::Error`] in the same write
    /// that also sets [`Self::state`] to [`PlaybackState::Error`].
    pub error: RwSignal<Option<VideoError>>,
}

impl VideoPlayerHandle {
    /// Wrap `session`, seed every signal from its current
    /// [`crate::PlayerSession::snapshot`], and register the listener that
    /// keeps them current (see the module's *Threading* section).
    pub fn new(session: PlayerSession) -> Self {
        let session = Arc::new(session);
        let PlayerSnapshot {
            state,
            position,
            duration,
            video_size,
            error,
        } = session.snapshot();

        let state = RwSignal::new(state);
        let position = RwSignal::new(position);
        let duration = RwSignal::new(duration);
        let video_size = RwSignal::new(video_size);
        let error = RwSignal::new(error);

        // Runs on the platform main thread (module doc's *Threading*
        // section): every arm only writes a signal, never reads one and
        // never calls back into `session`.
        let listener = session.set_listener(Box::new(move |event| match event {
            PlayerEvent::StateChanged(new_state) => state.set(new_state),
            PlayerEvent::Position {
                position: new_position,
                duration: new_duration,
            } => {
                position.set(new_position);
                duration.set(new_duration);
            }
            PlayerEvent::VideoSize { width, height } => {
                video_size.set(Some((width, height)));
            }
            PlayerEvent::Error(new_error) => {
                // Mirrors `snapshot::Shared::publish`'s own
                // error-then-state write order, so a `build` reading both
                // signals never sees `Error` with no error to report, nor
                // an error alongside a stale non-`Error` state.
                error.set(Some(new_error));
                state.set(PlaybackState::Error);
            }
        }));

        Self {
            session,
            state,
            position,
            duration,
            video_size,
            error,
            listener: Arc::new(listener),
        }
    }

    /// The wrapped session — for reading [`crate::PlayerSession::view_type`]/
    /// [`crate::PlayerSession::params_json`] (see [`super::video_view`]) or
    /// issuing a control call this handle has no pass-through for.
    pub fn session(&self) -> &PlayerSession {
        &self.session
    }

    /// The session's current state, read directly rather than through the
    /// signals — equivalent to reading [`Self::state`]/[`Self::position`]/
    /// [`Self::duration`]/[`Self::video_size`]/[`Self::error`] together, but
    /// untracked: prefer the signals from a `build` that must react to a
    /// later change.
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.session.snapshot()
    }

    /// Start (or resume) playback. See [`crate::PlayerSession::play`] for
    /// the error contract.
    pub fn play(&self) -> Result<(), VideoError> {
        self.session.play()
    }

    /// Pause playback, keeping the position. See
    /// [`crate::PlayerSession::pause`].
    pub fn pause(&self) -> Result<(), VideoError> {
        self.session.pause()
    }

    /// Seek to `position`. See [`crate::PlayerSession::seek_to`].
    pub fn seek_to(&self, position: Duration) -> Result<(), VideoError> {
        self.session.seek_to(position)
    }

    /// Set the playback rate. See [`crate::PlayerSession::set_rate`].
    pub fn set_rate(&self, rate: f32) -> Result<(), VideoError> {
        self.session.set_rate(rate)
    }

    /// Set the output volume. See [`crate::PlayerSession::set_volume`].
    pub fn set_volume(&self, volume: f32) -> Result<(), VideoError> {
        self.session.set_volume(volume)
    }

    /// Turn looping on or off. See [`crate::PlayerSession::set_looping`].
    pub fn set_looping(&self, looping: bool) -> Result<(), VideoError> {
        self.session.set_looping(looping)
    }

    /// Release the platform player. See [`crate::PlayerSession::close`] —
    /// idempotent, and distinct from dropping this handle (this struct's
    /// own doc's *Dropping this handle* section).
    pub fn close(&self) {
        self.session.close();
    }
}

/// [`VideoPlayerHandle::new`], plus an [`frust::on_cleanup`] registration —
/// the convenience for calling straight from a [`frust::Component`]'s
/// `init`/`build`: an app that stores the returned handle in its own
/// `Component::State` gets the listener explicitly unregistered at that
/// component's teardown, rather than relying only on `State`'s own eventual
/// drop.
///
/// This registers cleanup on the **currently ambient reactive owner** — call
/// it only from inside a component's `init`/`build`, matching every other
/// `on_cleanup`-registering call in this codebase (e.g. `frust_i18n`'s
/// `provide_i18n`, `examples/playground`'s `CameraPage::init`). Calling it
/// with no ambient owner is a harmless no-op registration (per
/// [`frust::on_cleanup`]'s own contract) — the returned handle still works,
/// it just won't be auto-unregistered.
///
/// Because the registered cleanup holds its own reference to the same
/// listener registration the returned handle holds (see
/// [`VideoPlayerHandle`]'s `listener` field doc — this is *why* it is
/// `Arc`-shared rather than a bare, uniquely-owned
/// [`crate::ListenerHandle`]), dropping the returned handle *before* the
/// component tears down does not by itself unregister the listener: the
/// registration ends only once both references are gone, which in ordinary
/// usage (the handle lives in `Component::State` until teardown) happens at
/// the same moment either way.
pub fn use_video_player(session: PlayerSession) -> VideoPlayerHandle {
    let handle = VideoPlayerHandle::new(session);
    let cleanup_listener = Arc::clone(&handle.listener);
    on_cleanup(move || drop(cleanup_listener));
    handle
}

#[cfg(test)]
mod tests {
    use frust::Get;

    use super::*;
    use crate::{VideoPlayer, VideoSource};

    /// A session over an ordinary remote source, with default options —
    /// mirrors `crate::conformance`'s own `open` helper (kept local per the
    /// plan note: this is the crate's first `api`-tier test module).
    fn open() -> PlayerSession {
        VideoPlayer::open(
            VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
            crate::PlayerOptions::default(),
        )
        .expect("the mock backend opens an ordinary source")
    }

    #[test]
    fn a_new_handle_is_seeded_from_the_sessions_loading_snapshot() {
        let handle = VideoPlayerHandle::new(open());

        assert_eq!(handle.state.get(), PlaybackState::Loading);
        assert_eq!(handle.position.get(), Duration::ZERO);
        assert_eq!(handle.duration.get(), None);
        assert_eq!(handle.video_size.get(), None);
        assert_eq!(handle.error.get(), None);
    }

    #[test]
    fn published_events_update_the_matching_signals() {
        let handle = VideoPlayerHandle::new(open());
        let host = handle.session().mock_host();

        host.emit_state(PlaybackState::Playing);
        assert_eq!(handle.state.get(), PlaybackState::Playing);

        host.emit_position(Duration::from_millis(1_500), Some(Duration::from_secs(10)));
        assert_eq!(handle.position.get(), Duration::from_millis(1_500));
        assert_eq!(handle.duration.get(), Some(Duration::from_secs(10)));

        host.emit_video_size(1_280, 720);
        assert_eq!(handle.video_size.get(), Some((1_280, 720)));

        // -4 is the frozen host code for a transport failure (see
        // `crate::conformance`'s identical fixture).
        host.emit_error(-4, "connection reset");
        assert_eq!(
            handle.error.get(),
            Some(VideoError::Network("connection reset".to_owned()))
        );
        // The state signal mirrors `PlayerSnapshot`'s own coupling: an
        // error also reads back as `PlaybackState::Error`.
        assert_eq!(handle.state.get(), PlaybackState::Error);
    }

    #[test]
    fn dropping_the_handle_stops_updates() {
        let handle = VideoPlayerHandle::new(open());
        let host = handle.session().mock_host();
        // `RwSignal` is `Copy`: taking a copy before dropping the handle
        // keeps the underlying arena slot readable, so the test can prove
        // silence rather than merely losing its only handle onto the value.
        let state = handle.state;

        host.emit_state(PlaybackState::Playing);
        assert_eq!(state.get(), PlaybackState::Playing);

        drop(handle);
        host.emit_state(PlaybackState::Paused);

        // The listener was unregistered on drop, so the later event never
        // reached the signal.
        assert_eq!(state.get(), PlaybackState::Playing);
    }

    #[test]
    fn use_video_player_builds_a_working_handle_with_no_ambient_owner() {
        // No reactive `Owner` is installed in this test (module doc's
        // rationale): `frust::on_cleanup` is documented to no-op without
        // one, so this only exercises that the convenience still returns a
        // fully working handle in that configuration.
        let handle = use_video_player(open());
        let host = handle.session().mock_host();

        host.emit_state(PlaybackState::Buffering);

        assert_eq!(handle.state.get(), PlaybackState::Buffering);
    }
}
