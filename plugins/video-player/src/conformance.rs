//! The platform-independent contract, checked on every host against the
//! in-memory player ([`crate::mock`]).
//!
//! Everything asserted here is behaviour an app can rely on regardless of
//! which backend is underneath: what a freshly opened session reports, that
//! commands reach the backend in the order they were issued, that published
//! events update the snapshot before they reach the listener, the
//! one-listener-per-session rule and its stale-handle safety, the
//! re-entrancy refusal, close/drop idempotence, and the frozen wire
//! spellings (state codes, error codes, `viewType`s, params keys). A device
//! gate proves the backends; this proves the contract they implement.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::mock::Command;
use crate::{
    PlaybackState, PlayerEvent, PlayerOptions, PlayerSession, PlayerSnapshot, VIEW_TYPE,
    VideoError, VideoFit, VideoPlayer, VideoSource, contract, params_json_with,
};

/// A session over an ordinary remote source, with default options.
fn open() -> PlayerSession {
    VideoPlayer::open(
        VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
        PlayerOptions::default(),
    )
    .expect("the mock backend opens an ordinary source")
}

/// A listener recording every event it is handed, plus the handle keeping it
/// registered.
fn recording_listener(
    session: &PlayerSession,
) -> (Arc<Mutex<Vec<PlayerEvent>>>, crate::ListenerHandle) {
    let events: Arc<Mutex<Vec<PlayerEvent>>> = Arc::default();
    let sink = Arc::clone(&events);
    let handle = session.set_listener(Box::new(move |event| {
        sink.lock().expect("listener sink").push(event);
    }));
    (events, handle)
}

#[test]
fn an_opened_session_is_loading_with_nothing_known_yet() {
    let session = open();

    assert_eq!(
        session.snapshot(),
        PlayerSnapshot {
            state: PlaybackState::Loading,
            position: Duration::ZERO,
            duration: None,
            video_size: None,
            error: None,
        }
    );
}

#[test]
fn open_hands_the_source_and_options_to_the_backend() {
    let options = PlayerOptions {
        autoplay: true,
        looping: true,
        volume: 0.25,
        mix_with_others: true,
    };
    let session = VideoPlayer::open(VideoSource::Asset("clip.mp4".to_owned()), options)
        .expect("an asset source opens");
    let host = session.mock_host();

    assert_eq!(host.source(), VideoSource::Asset("clip.mp4".to_owned()));
    let seen = host.options();
    assert!(seen.autoplay);
    assert!(seen.looping);
    assert_eq!(seen.volume, 0.25);
    assert!(seen.mix_with_others);
}

#[test]
fn a_source_the_platform_refuses_fails_the_open() {
    let error = VideoPlayer::open(VideoSource::Url(String::new()), PlayerOptions::default())
        .err()
        .expect("an empty source is refused");

    assert!(matches!(error, VideoError::UnsupportedSource(_)));
}

#[test]
fn commands_reach_the_backend_in_the_order_they_were_issued() {
    let session = open();
    let host = session.mock_host();

    session.play().expect("play");
    session.seek_to(Duration::from_millis(1_500)).expect("seek");
    session.set_rate(1.5).expect("rate");
    session.set_volume(0.5).expect("volume");
    session.set_looping(true).expect("looping");
    session.pause().expect("pause");

    assert_eq!(
        host.commands(),
        vec![
            Command::Play,
            Command::SeekTo(Duration::from_millis(1_500)),
            Command::SetRate(1.5),
            Command::SetVolume(0.5),
            Command::SetLooping(true),
            Command::Pause,
        ]
    );
}

#[test]
fn a_backend_refusal_reaches_the_caller() {
    let session = open();
    session
        .mock_host()
        .refuse_next(VideoError::Host("busy".to_owned()));

    assert_eq!(session.play(), Err(VideoError::Host("busy".to_owned())));
    // Armed once, not latched: the next command succeeds again.
    assert_eq!(session.pause(), Ok(()));
}

#[test]
fn published_events_update_the_snapshot() {
    let session = open();
    let host = session.mock_host();

    host.emit_state(PlaybackState::Playing);
    host.emit_video_size(1_920, 1_080);
    host.emit_position(Duration::from_millis(2_000), Some(Duration::from_secs(30)));

    assert_eq!(
        session.snapshot(),
        PlayerSnapshot {
            state: PlaybackState::Playing,
            position: Duration::from_millis(2_000),
            duration: Some(Duration::from_secs(30)),
            video_size: Some((1_920, 1_080)),
            error: None,
        }
    );
}

#[test]
fn a_duration_the_platform_does_not_know_stays_none() {
    let session = open();

    session
        .mock_host()
        .emit_position(Duration::from_millis(750), None);

    let snapshot = session.snapshot();
    assert_eq!(snapshot.position, Duration::from_millis(750));
    assert_eq!(snapshot.duration, None);
}

#[test]
fn an_error_event_fills_both_the_state_and_the_error_slot() {
    let session = open();
    let (events, _handle) = recording_listener(&session);

    // -4 is the frozen host code for a transport failure.
    session.mock_host().emit_error(-4, "connection reset");

    let snapshot = session.snapshot();
    assert_eq!(snapshot.state, PlaybackState::Error);
    assert_eq!(
        snapshot.error,
        Some(VideoError::Network("connection reset".to_owned()))
    );
    assert_eq!(
        *events.lock().expect("events"),
        vec![PlayerEvent::Error(VideoError::Network(
            "connection reset".to_owned()
        ))]
    );
}

#[test]
fn events_reach_the_listener_in_the_order_they_were_published() {
    let session = open();
    let host = session.mock_host();
    let (events, _handle) = recording_listener(&session);

    host.emit_state(PlaybackState::Buffering);
    host.emit_position(Duration::from_millis(10), None);
    host.emit_video_size(640, 360);
    host.emit_state(PlaybackState::Playing);

    assert_eq!(
        *events.lock().expect("events"),
        vec![
            PlayerEvent::StateChanged(PlaybackState::Buffering),
            PlayerEvent::Position {
                position: Duration::from_millis(10),
                duration: None,
            },
            PlayerEvent::VideoSize {
                width: 640,
                height: 360,
            },
            PlayerEvent::StateChanged(PlaybackState::Playing),
        ]
    );
}

#[test]
fn the_snapshot_is_already_updated_when_the_listener_runs() {
    let session = open();
    let host = session.mock_host();
    let seen: Arc<Mutex<Vec<PlaybackState>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let session = Arc::new(session);
    let weak = Arc::downgrade(&session);
    let _handle = session.set_listener(Box::new(move |_| {
        if let Some(session) = weak.upgrade() {
            sink.lock().expect("sink").push(session.snapshot().state);
        }
    }));

    host.emit_state(PlaybackState::Playing);

    assert_eq!(*seen.lock().expect("seen"), vec![PlaybackState::Playing]);
}

#[test]
fn a_second_listener_replaces_the_first() {
    let session = open();
    let (first, first_handle) = recording_listener(&session);
    let (second, _second_handle) = recording_listener(&session);

    session.mock_host().emit_state(PlaybackState::Playing);

    assert!(first.lock().expect("first").is_empty());
    assert_eq!(second.lock().expect("second").len(), 1);

    // The replaced registration's handle is inert: dropping it must not
    // unregister the listener that replaced it.
    drop(first_handle);
    session.mock_host().emit_state(PlaybackState::Paused);
    assert_eq!(second.lock().expect("second").len(), 2);
}

#[test]
fn dropping_the_handle_unregisters_the_listener() {
    let session = open();
    let (events, handle) = recording_listener(&session);

    session.mock_host().emit_state(PlaybackState::Playing);
    drop(handle);
    session.mock_host().emit_state(PlaybackState::Paused);

    assert_eq!(events.lock().expect("events").len(), 1);
}

#[test]
fn remove_unregisters_the_listener_explicitly() {
    let session = open();
    let (events, handle) = recording_listener(&session);

    handle.remove();
    session.mock_host().emit_state(PlaybackState::Playing);

    assert!(events.lock().expect("events").is_empty());
}

#[test]
fn a_handle_outliving_its_session_unregisters_nothing() {
    let session = open();
    let (_events, handle) = recording_listener(&session);

    drop(session);
    // The session's shared state is gone; the handle's drop must not reach
    // for it.
    drop(handle);
}

#[test]
fn a_control_call_from_inside_the_listener_is_refused() {
    let session = Arc::new(open());
    let host = session.mock_host();
    let outcome: Arc<Mutex<Option<Result<(), VideoError>>>> = Arc::default();
    let sink = Arc::clone(&outcome);
    // Weak, so the listener the session owns doesn't keep the session alive.
    let weak = Arc::downgrade(&session);
    let _handle = session.set_listener(Box::new(move |_| {
        if let Some(session) = weak.upgrade() {
            *sink.lock().expect("outcome") = Some(session.play());
        }
    }));

    host.emit_state(PlaybackState::Playing);

    assert_eq!(
        *outcome.lock().expect("outcome"),
        Some(Err(VideoError::Reentrant))
    );
    // The refusal happened before the backend saw anything.
    assert_eq!(host.commands(), Vec::new());
    // And the flag is per-delivery, not sticky: a later call succeeds.
    assert_eq!(session.play(), Ok(()));
}

#[test]
fn closing_from_inside_the_listener_delivers_idle_after_the_outer_event() {
    let session = Arc::new(open());
    let host = session.mock_host();
    let states: Arc<Mutex<Vec<PlaybackState>>> = Arc::default();
    let depth: Arc<AtomicUsize> = Arc::default();
    let max_depth: Arc<AtomicUsize> = Arc::default();
    let sink = Arc::clone(&states);
    let depth_sink = Arc::clone(&depth);
    let max_depth_sink = Arc::clone(&max_depth);
    let weak = Arc::downgrade(&session);
    let _handle = session.set_listener(Box::new(move |event| {
        let nested = depth_sink.fetch_add(1, Ordering::SeqCst) + 1;
        max_depth_sink.fetch_max(nested, Ordering::SeqCst);

        if let PlayerEvent::StateChanged(state) = event {
            sink.lock().expect("states").push(state);
            if state == PlaybackState::Playing
                && let Some(session) = weak.upgrade()
            {
                session.close();
            }
        }

        depth_sink.fetch_sub(1, Ordering::SeqCst);
    }));

    host.emit_state(PlaybackState::Playing);

    assert_eq!(
        *states.lock().expect("states"),
        vec![PlaybackState::Playing, PlaybackState::Idle]
    );
    // The listener was never re-entered: the close's own publish waited for
    // the outer call to return rather than running on top of it.
    assert_eq!(max_depth.load(Ordering::SeqCst), 1);
    assert_eq!(session.snapshot().state, PlaybackState::Idle);
}

#[test]
fn a_control_call_after_an_inline_close_is_still_refused_inside_the_listener() {
    let session = Arc::new(open());
    let host = session.mock_host();
    let outcome: Arc<Mutex<Option<Result<(), VideoError>>>> = Arc::default();
    let sink = Arc::clone(&outcome);
    let weak = Arc::downgrade(&session);
    let _handle = session.set_listener(Box::new(move |event| {
        if matches!(event, PlayerEvent::StateChanged(PlaybackState::Playing))
            && let Some(session) = weak.upgrade()
        {
            session.close();
            // The nested close published `Idle` behind the scenes, but this
            // thread is still inside the outer `Playing` delivery — a
            // control call here must still be refused as re-entrant, not
            // as merely-closed.
            *sink.lock().expect("outcome") = Some(session.play());
        }
    }));

    host.emit_state(PlaybackState::Playing);

    assert_eq!(
        *outcome.lock().expect("outcome"),
        Some(Err(VideoError::Reentrant))
    );
}

#[test]
fn the_delivering_flag_is_clear_after_a_nested_publish() {
    let session = Arc::new(open());
    let host = session.mock_host();
    let weak = Arc::downgrade(&session);
    let _handle = session.set_listener(Box::new(move |event| {
        if matches!(event, PlayerEvent::StateChanged(PlaybackState::Playing))
            && let Some(session) = weak.upgrade()
        {
            session.close();
        }
    }));

    host.emit_state(PlaybackState::Playing);

    // A second, unrelated session on the same thread is not caught by
    // whatever the first session's nested publish left behind.
    let other = open();
    assert_eq!(other.play(), Ok(()));
}

#[test]
fn close_is_idempotent_and_closes_exactly_once() {
    let session = open();
    let host = session.mock_host();

    session.close();
    session.close();
    drop(session);

    assert_eq!(host.commands(), vec![Command::Close]);
}

#[test]
fn dropping_a_session_closes_it() {
    let session = open();
    let host = session.mock_host();

    drop(session);

    assert_eq!(host.commands(), vec![Command::Close]);
}

#[test]
fn every_control_call_after_close_reports_closed() {
    let session = open();
    session.close();

    assert_eq!(session.play(), Err(VideoError::Closed));
    assert_eq!(session.pause(), Err(VideoError::Closed));
    assert_eq!(session.seek_to(Duration::ZERO), Err(VideoError::Closed));
    assert_eq!(session.set_rate(2.0), Err(VideoError::Closed));
    assert_eq!(session.set_volume(0.1), Err(VideoError::Closed));
    assert_eq!(session.set_looping(false), Err(VideoError::Closed));
    // A closed session reads Idle — the backend publishes it as the last
    // thing its own close does.
    assert_eq!(session.snapshot().state, PlaybackState::Idle);
}

#[test]
fn closing_publishes_idle_to_the_snapshot_and_listener() {
    let session = open();
    let (events, _handle) = recording_listener(&session);

    session.close();

    assert_eq!(session.snapshot().state, PlaybackState::Idle);
    assert_eq!(
        *events.lock().expect("events"),
        vec![PlayerEvent::StateChanged(PlaybackState::Idle)]
    );
}

#[test]
fn a_second_close_publishes_nothing_more() {
    let session = open();
    let (events, _handle) = recording_listener(&session);

    session.close();
    session.close();
    drop(session);

    // Idempotent at the wrapper: the backend's own close — and so its Idle
    // publish — runs exactly once, matching `close_is_idempotent_and_closes_exactly_once`.
    assert_eq!(
        *events.lock().expect("events"),
        vec![PlayerEvent::StateChanged(PlaybackState::Idle)]
    );
}

#[test]
fn playback_state_codes_are_the_frozen_table() {
    let table = [
        (0, PlaybackState::Idle),
        (1, PlaybackState::Loading),
        (2, PlaybackState::Paused),
        (3, PlaybackState::Playing),
        (4, PlaybackState::Buffering),
        (5, PlaybackState::Ended),
        (6, PlaybackState::Error),
    ];

    for (code, state) in table {
        assert_eq!(PlaybackState::from_code(code), Some(state));
        assert_eq!(u32::from(state.code()), u32::try_from(code).expect("code"));
    }
    // A host newer than this crate degrades rather than panicking.
    assert_eq!(PlaybackState::from_code(7), None);
    assert_eq!(PlaybackState::from_code(-1), None);
}

#[test]
fn host_error_codes_are_the_frozen_table() {
    assert_eq!(
        VideoError::from_host_code(-1, "gone"),
        VideoError::UnknownSession
    );
    assert_eq!(
        VideoError::from_host_code(-2, "bad url"),
        VideoError::UnsupportedSource("bad url".to_owned())
    );
    assert_eq!(
        VideoError::from_host_code(-3, "no host"),
        VideoError::PlatformNotInitialized
    );
    assert_eq!(
        VideoError::from_host_code(-4, "offline"),
        VideoError::Network("offline".to_owned())
    );
    assert_eq!(
        VideoError::from_host_code(-5, "codec"),
        VideoError::Decoder("codec".to_owned())
    );
    // Anything else keeps the host's message rather than being dropped.
    assert_eq!(
        VideoError::from_host_code(-99, "something else"),
        VideoError::Host("something else".to_owned())
    );
}

#[test]
fn player_options_default_to_a_silent_start_at_full_volume() {
    let options = PlayerOptions::default();

    assert!(!options.autoplay);
    assert!(!options.looping);
    assert_eq!(options.volume, 1.0);
    assert!(!options.mix_with_others);
    assert_eq!(VideoFit::default(), VideoFit::Contain);
}

#[test]
fn the_params_payload_spells_each_platform_its_own_way() {
    assert_eq!(
        params_json_with(contract::ANDROID_SESSION_KEY, 7, VideoFit::Contain),
        r#"{"sessionId":7,"fit":"contain"}"#
    );
    assert_eq!(
        params_json_with(contract::APPLE_SESSION_KEY, 7, VideoFit::Cover),
        r#"{"session":7,"fit":"cover"}"#
    );
}

#[test]
fn a_sessions_params_carry_its_own_id_and_this_targets_key() {
    let session = open();
    let id = session.mock_host().id();

    assert_eq!(
        session.params_json(VideoFit::Cover),
        params_json_with(crate::SESSION_KEY, id, VideoFit::Cover)
    );
}

#[test]
fn the_view_type_is_target_gated_never_one_shared_literal() {
    assert_eq!(
        contract::ANDROID_VIEW_TYPE,
        "dev.frust.videoplayer.VideoPlayerViewFactory"
    );
    assert_eq!(contract::APPLE_VIEW_TYPE, "VideoPlayerViewFactory");
    assert_ne!(contract::ANDROID_VIEW_TYPE, contract::APPLE_VIEW_TYPE);

    let session = open();
    assert_eq!(session.view_type(), VIEW_TYPE);
}

/// Compiled only where there is no backend at all — i.e. neither the Android
/// nor the Apple gate, and not a macOS `cargo test` host either (macOS is
/// `target_vendor = "apple"`, so it builds `crate::apple`). A plain
/// Linux/Windows host runs it.
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
#[test]
fn the_no_backend_arm_fails_soft_rather_than_panicking() {
    use crate::backend::PlayerBackend;
    use crate::snapshot::Shared;
    use crate::unsupported::UnsupportedBackend;

    let opened = UnsupportedBackend::open(
        &VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
        &PlayerOptions::default(),
        Arc::new(Shared::new()),
    );

    assert!(matches!(opened.map(|_| ()), Err(VideoError::NotSupported)));
    assert_eq!(VIEW_TYPE, "");
}
