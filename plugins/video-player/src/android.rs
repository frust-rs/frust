//! The Android backend — a `dev.frust.videoplayer.FrustVideoPlayerHost`
//! Kotlin host (`plugins/video-player/platform/android/`) driven over this
//! crate's own JNI surface: every player command goes out as a static-method
//! call on that class, and every answer comes back through the four
//! `nativeOn*` exports below, each publishing into its session's
//! [`crate::snapshot::Shared`].
//!
//! # The frozen contract
//!
//! **`FrustVideoPlayerHost.kt` and this module build to this table —
//! changing it means updating both files together.** `dev.frust.videoplayer`
//! is a subpackage of the embedding module's `dev.frust` (`dev.frust` is
//! `frust-embedding`'s exclusive package — `docs/PLUGINS_CODE_STANDARDS.md`'s
//! Plugin Conventions); the package is baked into every JNI export symbol
//! below, so `FrustVideoPlayerHost` may never move once shipped.
//!
//! ## Rust → Kotlin (static methods on `FrustVideoPlayerHost`, resolved via
//! `context.getClassLoader().loadClass(...)` — the `FrustCameraHost`
//! mechanism, `plugins/camera/src/android.rs`)
//!
//! Each row carries the Java parameter list **and** the JNI descriptor this
//! module actually calls with; the two are one statement, and the descriptor
//! is what a device gate diffs against the Kotlin declaration.
//!
//! | Method | Parameters (Java) | JNI descriptor | Returns |
//! |---|---|---|---|
//! | `openPlayer` | `(int sourceKind, String source, boolean autoplay, boolean looping, float volume)` | `(ILjava/lang/String;ZZF)I` | ≥0 session id; <0 an open error code (below) |
//! | `play` | `(int session)` | `(I)I` | [`COMMAND_OK`] / [`COMMAND_UNKNOWN_SESSION`] |
//! | `pause` | `(int session)` | `(I)I` | as `play` |
//! | `seekTo` | `(int session, long positionMs)` | `(IJ)I` | as `play` |
//! | `setRate` | `(int session, float rate)` | `(IF)I` | as `play` |
//! | `setVolume` | `(int session, float volume)` | `(IF)I` | as `play` |
//! | `setLooping` | `(int session, boolean looping)` | `(IZ)I` | as `play` |
//! | `close` | `(int session)` | `(I)I` | as `play`; a repeat close answers [`COMMAND_UNKNOWN_SESSION`] |
//!
//! `sourceKind` is [`SOURCE_KIND_FILE`] `0` (an **absolute** filesystem
//! path), [`SOURCE_KIND_ASSET`] `1` (a path relative to the app's `assets/`
//! directory, not a filesystem path), or [`SOURCE_KIND_URL`] `2`.
//!
//! `openPlayer`'s negative returns are the crate-wide host-error codes
//! [`VideoError::from_host_code`] decodes: `-1` unknown session, `-2`
//! unsupported source, `-3` host not initialized, `-4` network, `-5`
//! decoder. They carry no message of their own — the host reports an int, so
//! this module synthesizes the message ([`open_error`]) from the source it
//! asked for. A failure that happens *after* the open lands arrives instead
//! as `nativeOnError`, which does carry one.
//!
//! [`crate::PlayerOptions::mix_with_others`] is deliberately **absent** from
//! the contract: it configures an iOS `AVAudioSession` and Android has no
//! equivalent, so the gap is recorded per-platform rather than emulated
//! (`docs/PLUGINS_CODE_STANDARDS.md`'s capability-gap rule).
//!
//! ## Kotlin → Rust (this crate's own `#[unsafe(no_mangle)]` JNI exports)
//!
//! - [`Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnState`]`(env, class, session: jint, state: jint)`
//!   — `0` Idle / `1` Loading / `2` Paused / `3` Playing / `4` Buffering /
//!   `5` Ended / `6` Error, decoded by [`PlaybackState::from_code`]. The
//!   **initial** Loading is owned by this module, not the host: [`AndroidBackend::open`]
//!   publishes it itself right after registering the session (matching
//!   `apple.rs`), so the post-open snapshot is `Loading` on every thread
//!   regardless of when — or whether — the host's own first callback lands.
//!   A state the host reports that already equals the snapshot's current
//!   state, Loading included, is therefore a harmless duplicate and is
//!   dropped rather than redelivered — see [`should_publish_state`].
//! - [`Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnPosition`]`(env, class, session: jint, positionMs: jlong, durationMs: jlong)`
//!   — `durationMs` is [`DURATION_UNKNOWN`] (`-1`), or any negative value,
//!   while the duration is not known (a live stream never resolves one).
//! - [`Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnVideoSize`]`(env, class, session: jint, width: jint, height: jint)`
//!   — decoded pixel geometry; a non-positive pair is dropped rather than
//!   published as a zero size.
//! - [`Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnError`]`(env, class, session: jint, code: jint, message: JString)`
//!   — `code` is the same host-error table `openPlayer` reports, this time
//!   with the host's own message.
//!
//! All four are called on the **main thread** and carry a live JNI frame, so
//! none of them attaches: each upgrades its [`jni::EnvUnowned`] via
//! [`jni::EnvUnowned::with_env`], which wraps the body in `catch_unwind` —
//! the crate's own no-unwind-across-FFI guarantee
//! (`docs/CODE_STANDARDS.md`'s Language Idioms). This module holds **no
//! `unsafe` block at all**; its only `unsafe` tokens are the four exports'
//! `#[unsafe(no_mangle)]` attributes.
//!
//! # The platform-view spellings
//!
//! The Android `viewType` is `dev.frust.videoplayer.VideoPlayerViewFactory`
//! and the params payload is `{"sessionId":N,"fit":"contain"|"cover"}` —
//! both frozen, and both parsed by the Kotlin factory. Neither literal is
//! spelled here: [`crate::VIEW_TYPE`] and
//! [`crate::PlayerSession::params_json`] already own them (target-gated at
//! the crate root, with the Apple spellings beside them), so this module
//! states the contract and the crate root remains the single definition.
//!
//! # Session registry
//!
//! [`SESSIONS`] is the process-wide `session id -> `[`Arc<Shared>`](Shared)
//! map the exports publish through. [`AndroidBackend::open`] inserts an
//! entry the moment `openPlayer` answers — **before** returning the session,
//! since the host starts loading immediately and its first state callback
//! may land while `open` is still returning — then immediately publishes the
//! initial `Loading` itself (see the contract table above), and
//! [`AndroidSession::close`] removes it *after* `close` returns, so a
//! callback the host emits during its own teardown still reaches the
//! snapshot. `Idle` is published by whichever side reaches the snapshot
//! first — the host's inline teardown callback on a main-thread close, or
//! Rust's post-removal publish otherwise — and never twice; a closed
//! session's snapshot reads `Idle` on this backend just as it does on
//! `apple.rs` and the mock.
//!
//! An [`AndroidSession`] itself holds only the integer id plus its own
//! [`Shared`] handle, and no JNI reference at all: every command re-attaches
//! for the length of that one call ([`with_host`]), the scoped-attach rule
//! `docs/PLUGINS_CODE_STANDARDS.md` states (threads do not auto-detach, so a
//! permanent attach leaks).
//!
//! A callback is dispatched by cloning the `Arc` **out** of the map and
//! releasing the lock before publishing: a listener is allowed to close its
//! own session from inside an event ([`crate::PlayerSession::close`]'s doc),
//! and that close takes the same lock.
//!
//! # Fail-soft, never a panic
//!
//! Every command routes through [`with_host`], which checks `frust-plugin`'s
//! readiness first: before the host shell installs the `(JavaVM, Context)`
//! handles, and when `FrustVideoPlayerHost` cannot be loaded at all (the
//! plugin's Gradle module isn't wired into the app), calls report
//! [`VideoError::PlatformNotInitialized`] — the variant whose own doc names
//! both causes — with the underlying JNI detail logged. Any other JNI
//! failure maps to [`VideoError::Host`] with the failing operation named.
//! No path outside the unit tests calls `unwrap`/`expect`: a conversion
//! that could fail is given a total fallback instead, and a poisoned
//! registry lock is recovered from rather than propagated ([`lock`]).
//!
//! # What is tested, and where
//!
//! The JNI paths themselves are exercised only by the Android device gate —
//! nothing off-device can call a `FrustVideoPlayerHost` that does not exist.
//! So every decision this backend makes that is *not* a JNI call is a pure
//! function ([`source_args`], [`open_error`], [`command_result`],
//! [`position_arg`], [`position_event`], [`video_size_event`]), named and
//! unit-tested below, and the JNI bodies are left as thin call sites over
//! them.

// Under `cfg(test)` `crate::backend::Active` selects the in-memory mock on
// every target, including Android, so the backend and session types here
// have no caller in a test build — the suite drives the fake by design (see
// `crate::backend::Active`'s own doc).
#![cfg_attr(test, allow(dead_code))]

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::backend::{BackendSession, PlayerBackend};
use crate::snapshot::Shared;
use crate::{PlaybackState, PlayerEvent, PlayerOptions, VideoError, VideoSource};

/// The Kotlin host's fully-qualified class name in **binary/dotted** form, as
/// `ClassLoader.loadClass` expects (**not** the slash form `FindClass` wants)
/// — the module doc's frozen contract, and the package baked into every JNI
/// export symbol below.
const HOST_CLASS_BINARY: &str = "dev.frust.videoplayer.FrustVideoPlayerHost";

/// `openPlayer`'s `sourceKind` for [`VideoSource::File`] — an **absolute**
/// filesystem path (contract table).
const SOURCE_KIND_FILE: i32 = 0;
/// `openPlayer`'s `sourceKind` for [`VideoSource::Asset`] — a path relative
/// to the app's `assets/` directory, resolved by the host's `AssetManager`.
const SOURCE_KIND_ASSET: i32 = 1;
/// `openPlayer`'s `sourceKind` for [`VideoSource::Url`].
const SOURCE_KIND_URL: i32 = 2;

/// Every command method's "accepted" return code (contract table).
const COMMAND_OK: i32 = 0;
/// Every command method's "this host knows no such session" return code —
/// [`VideoError::UnknownSession`], and what a repeat `close` answers.
const COMMAND_UNKNOWN_SESSION: i32 = -1;

/// `nativeOnPosition`'s `durationMs` sentinel for "not known yet" (contract
/// table). Any negative value reads the same way.
const DURATION_UNKNOWN: i64 = -1;

/// The cached `dev.frust.videoplayer.FrustVideoPlayerHost` class reference.
///
/// **One** global reference for the whole process (plus the method ids ART
/// caches behind `call_static_method`), never a per-call one: ART's
/// global-ref table is a hard-capped budget, and the position callback path
/// makes this a hot class. A local `JClass` cannot be cached instead —
/// locals die with their JNI frame.
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// The process-wide `session id -> `[`Shared`] map (module doc's *Session
/// registry*): written by [`AndroidBackend::open`] and
/// [`AndroidSession::close`], read by the four `nativeOn*` exports.
///
/// `HashMap::new` is not `const`, so this is a [`LazyLock`] rather than a
/// bare `static`.
static SESSIONS: LazyLock<Mutex<HashMap<i32, Arc<Shared>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Opens Android player sessions.
pub(crate) struct AndroidBackend;

impl PlayerBackend for AndroidBackend {
    type Session = AndroidSession;

    /// `FrustVideoPlayerHost.openPlayer` — hands the source to the host and
    /// returns as soon as it answers with a session id, without waiting for
    /// the item to load (the crate doc's threading contract). Loading
    /// progress arrives afterwards through the `nativeOn*` exports.
    ///
    /// # Errors
    /// [`VideoError::UnsupportedSource`] for a source this backend refuses
    /// before calling at all (a relative [`VideoSource::File`] path, or one
    /// that is not valid UTF-8 — see [`source_args`]);
    /// [`VideoError::PlatformNotInitialized`] when the platform handles or
    /// the host class are missing; whatever [`VideoError::from_host_code`]
    /// decodes from a negative `openPlayer` return; [`VideoError::Host`] for
    /// a JNI failure.
    fn open(
        source: &VideoSource,
        options: &PlayerOptions,
        shared: Arc<Shared>,
    ) -> Result<Self::Session, VideoError> {
        let (kind, value) = source_args(source)?;
        let autoplay = jboolean::from(options.autoplay);
        let looping = jboolean::from(options.looping);
        let volume: jfloat = options.volume;

        let code = with_host(|env, class| {
            run_jni(env, "FrustVideoPlayerHost.openPlayer", |env| {
                let source_jstr = env.new_string(value)?;
                env.call_static_method(
                    class,
                    jni_str!("openPlayer"),
                    jni_sig!("(ILjava/lang/String;ZZF)I"),
                    &[
                        JValue::Int(kind),
                        JValue::Object(&source_jstr),
                        JValue::Bool(autoplay),
                        JValue::Bool(looping),
                        JValue::Float(volume),
                    ],
                )?
                .i()
            })
        })?;

        if code < 0 {
            return Err(open_error(code, kind, value));
        }

        // Registered before the session is handed back: the host is already
        // loading, so its first state callback can land while this call is
        // still returning (module doc's *Session registry*).
        lock(&SESSIONS).insert(code, Arc::clone(&shared));

        // Published from Rust, matching `apple.rs`'s `open`: the post-open
        // snapshot must be `Loading` regardless of what the host managed to
        // deliver by the time this call returns. No listener can exist yet
        // either way — `PlayerSession` is not built until this call returns
        // — so this publish reaches nobody, and only seeds the snapshot. Any
        // Loading the host later reports for this session is a no-op
        // duplicate ([`should_publish_state`]).
        shared.publish(PlayerEvent::StateChanged(PlaybackState::Loading));

        Ok(AndroidSession { id: code, shared })
    }
}

/// One Android player session: the host's integer handle plus the snapshot
/// its callbacks publish into. Holds no JNI reference — see the module doc.
pub(crate) struct AndroidSession {
    /// The id the Kotlin side knows this session by, minted by `openPlayer`
    /// and carried in [`crate::PlayerSession::params_json`] so the native
    /// view can find the player to attach to.
    id: i32,
    /// This session's registry entry, kept so [`Self::close`] removes
    /// exactly its own — see that method.
    shared: Arc<Shared>,
}

impl AndroidSession {
    /// Call one `(session, ...) -> int` command on the host and map its
    /// return through [`command_result`].
    ///
    /// `call` receives the session id rather than closing over `self` so the
    /// whole JNI body stays a single expression at each call site, where its
    /// own `jni_sig!` descriptor is visible (module doc's contract table).
    fn command(
        &self,
        op: &str,
        call: impl FnOnce(
            &mut Env<'_>,
            &Global<JClass<'static>>,
            jint,
        ) -> Result<jint, jni::errors::Error>,
    ) -> Result<(), VideoError> {
        let code = with_host(|env, class| run_jni(env, op, |env| call(env, class, self.id)))?;
        command_result(op, code)
    }
}

impl BackendSession for AndroidSession {
    fn play(&self) -> Result<(), VideoError> {
        self.command("FrustVideoPlayerHost.play", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("play"),
                jni_sig!("(I)I"),
                &[JValue::Int(session)],
            )?
            .i()
        })
    }

    fn pause(&self) -> Result<(), VideoError> {
        self.command("FrustVideoPlayerHost.pause", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("pause"),
                jni_sig!("(I)I"),
                &[JValue::Int(session)],
            )?
            .i()
        })
    }

    fn seek_to(&self, position: Duration) -> Result<(), VideoError> {
        let position_ms: jlong = position_arg(position);
        self.command("FrustVideoPlayerHost.seekTo", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("seekTo"),
                jni_sig!("(IJ)I"),
                &[JValue::Int(session), JValue::Long(position_ms)],
            )?
            .i()
        })
    }

    fn set_rate(&self, rate: f32) -> Result<(), VideoError> {
        self.command("FrustVideoPlayerHost.setRate", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("setRate"),
                jni_sig!("(IF)I"),
                &[JValue::Int(session), JValue::Float(rate)],
            )?
            .i()
        })
    }

    fn set_volume(&self, volume: f32) -> Result<(), VideoError> {
        self.command("FrustVideoPlayerHost.setVolume", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("setVolume"),
                jni_sig!("(IF)I"),
                &[JValue::Int(session), JValue::Float(volume)],
            )?
            .i()
        })
    }

    fn set_looping(&self, looping: bool) -> Result<(), VideoError> {
        let looping = jboolean::from(looping);
        self.command("FrustVideoPlayerHost.setLooping", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("setLooping"),
                jni_sig!("(IZ)I"),
                &[JValue::Int(session), JValue::Bool(looping)],
            )?
            .i()
        })
    }

    /// Release the host's player, drop the registry entry, and conditionally
    /// publish `Idle` — the post-close contract every backend answers the same
    /// way (module doc's *Session registry*).
    ///
    /// Returns nothing, so a refusal is logged rather than reported — and a
    /// repeat call is harmless: the host answers
    /// [`COMMAND_UNKNOWN_SESSION`] for an id it has already released, which
    /// is exactly the debug-logged path below. The entry is removed **after**
    /// the call returns so a teardown callback the host emits from inside
    /// `close` still reaches the snapshot. `Idle` is published only when this
    /// call is the one that actually removed the entry, and only if the
    /// snapshot does not already read `Idle` (i.e., the host's inline teardown
    /// callback did not already move it there on the main thread). This
    /// prevents publishing `Idle` twice on a main-thread close: once when the
    /// host's inline `notifyState(STATE_IDLE)` reaches `nativeOnState`, then
    /// again after the entry is removed. A defensive match with the registry
    /// also keeps a reused id's newer session from having its own state
    /// overwritten by an older close.
    fn close(&self) {
        if let Err(err) = self.command("FrustVideoPlayerHost.close", |env, class, session| {
            env.call_static_method(
                class,
                jni_str!("close"),
                jni_sig!("(I)I"),
                &[JValue::Int(session)],
            )?
            .i()
        }) {
            log::debug!(
                "frust-video-player: closing session {} reported {err} — releasing it anyway",
                self.id
            );
        }

        let removed = {
            let mut sessions = lock(&SESSIONS);
            let is_this_session = sessions
                .get(&self.id)
                .is_some_and(|registered| Arc::ptr_eq(registered, &self.shared));
            if is_this_session {
                sessions.remove(&self.id);
            }
            is_this_session
        };
        if removed && should_publish_state(self.shared.snapshot().state, PlaybackState::Idle) {
            self.shared
                .publish(PlayerEvent::StateChanged(PlaybackState::Idle));
        }
    }

    fn id(&self) -> u32 {
        // `openPlayer` mints ids as non-negative `jint`s — a negative return
        // is an error code `open` already refused — so this conversion never
        // loses one. A nonsensical negative reports `0` rather than panicking
        // on a value that reached us from the host.
        u32::try_from(self.id).unwrap_or(0)
    }
}

// --- Pure helpers (no `jni` types: unit-tested below) -----------------------

/// Schemes a [`VideoSource::Url`] may name — every other scheme is refused by
/// [`source_args`] before the string ever reaches the host. A media URL is
/// the app data most likely to arrive from outside the trust boundary (a CMS
/// response, a deep link, a scanned QR code), and `file://`, `content://`,
/// `data:` and the like each reach somewhere neither this crate nor the host
/// chose to expose.
const URL_SCHEMES: [&str; 2] = ["http", "https"];

/// The scheme naming `url`, ASCII-case-insensitively — the substring before
/// its first `:` — or `None` when `url` has no `:` at all.
fn url_scheme(url: &str) -> Option<&str> {
    url.split_once(':').map(|(scheme, _)| scheme)
}

/// Map a [`VideoSource`] to the `(sourceKind, source)` pair `openPlayer`
/// takes (module doc's contract table).
///
/// # Errors
/// [`VideoError::UnsupportedSource`] for a relative [`VideoSource::File`]
/// path — the host resolves nothing, so a relative path would be interpreted
/// against whatever working directory the process happens to have — one that
/// is not valid UTF-8, which cannot cross into a Java `String` at all — or a
/// [`VideoSource::Url`] whose scheme is not in [`URL_SCHEMES`] (including one
/// with no scheme at all). The message names only the refused scheme, never
/// the full URL, since a refused URL is exactly the input most likely to
/// carry something worth not repeating into a log.
fn source_args(source: &VideoSource) -> Result<(i32, &str), VideoError> {
    match source {
        VideoSource::File(path) => {
            if !path.is_absolute() {
                return Err(VideoError::UnsupportedSource(format!(
                    "a file source must be an absolute path, got `{}`",
                    path.display()
                )));
            }
            path.to_str()
                .map(|path| (SOURCE_KIND_FILE, path))
                .ok_or_else(|| {
                    VideoError::UnsupportedSource(format!(
                        "a file source must be valid UTF-8, got `{}`",
                        path.display()
                    ))
                })
        }
        VideoSource::Asset(name) => Ok((SOURCE_KIND_ASSET, name)),
        VideoSource::Url(url) => match url_scheme(url) {
            Some(scheme)
                if URL_SCHEMES
                    .iter()
                    .any(|allowed| scheme.eq_ignore_ascii_case(allowed)) =>
            {
                Ok((SOURCE_KIND_URL, url.as_str()))
            }
            Some(scheme) => Err(VideoError::UnsupportedSource(format!(
                "unsupported URL scheme `{scheme}`"
            ))),
            None => Err(VideoError::UnsupportedSource(
                "a URL source must have a scheme".to_owned(),
            )),
        },
    }
}

/// Decode a negative `openPlayer` return, naming the source it refused.
///
/// The host answers with an int alone, so the message every
/// [`VideoError::from_host_code`] variant carries is synthesized here rather
/// than relayed — an open failure with a message comes back as
/// `nativeOnError` instead.
fn open_error(code: i32, source_kind: i32, source: &str) -> VideoError {
    VideoError::from_host_code(
        code,
        &format!(
            "android video backend: FrustVideoPlayerHost.openPlayer refused {} source `{source}` \
             (code {code})",
            source_kind_name(source_kind)
        ),
    )
}

/// A `sourceKind`'s name, for [`open_error`]'s message.
fn source_kind_name(source_kind: i32) -> &'static str {
    match source_kind {
        SOURCE_KIND_FILE => "file",
        SOURCE_KIND_ASSET => "asset",
        SOURCE_KIND_URL => "url",
        _ => "unknown",
    }
}

/// Map a command method's return code (module doc's contract table).
///
/// # Errors
/// [`VideoError::UnknownSession`] for [`COMMAND_UNKNOWN_SESSION`], and
/// [`VideoError::Host`] for a code outside the contract — a host newer than
/// this crate degrades to a readable message rather than a panic.
fn command_result(op: &str, code: i32) -> Result<(), VideoError> {
    match code {
        COMMAND_OK => Ok(()),
        COMMAND_UNKNOWN_SESSION => Err(VideoError::UnknownSession),
        other => Err(VideoError::Host(format!(
            "android video backend: {op} returned an unknown code {other}"
        ))),
    }
}

/// A seek position as the `long positionMs` `seekTo` takes, saturating
/// rather than wrapping: a nonsense position must not turn into a small one.
fn position_arg(position: Duration) -> i64 {
    i64::try_from(position.as_millis()).unwrap_or(i64::MAX)
}

/// Build the event `nativeOnPosition` publishes.
///
/// A negative `duration_ms` — [`DURATION_UNKNOWN`], and any other negative a
/// host might send — means "not known yet", which is `None` rather than a
/// zero duration. A negative position is impossible from a well-behaved host
/// and reads as zero.
fn position_event(position_ms: i64, duration_ms: i64) -> PlayerEvent {
    let duration = if duration_ms == DURATION_UNKNOWN {
        None
    } else {
        // Any *other* negative a host might send reads the same way, rather
        // than being saturated into a duration the item does not have.
        u64::try_from(duration_ms).ok().map(Duration::from_millis)
    };

    PlayerEvent::Position {
        position: Duration::from_millis(u64::try_from(position_ms).unwrap_or(0)),
        duration,
    }
}

/// Build the event `nativeOnVideoSize` publishes, or `None` for a
/// non-positive pair.
///
/// [`crate::PlayerSnapshot::video_size`] spells "not known yet" as `None`,
/// so a zero or negative dimension is dropped here rather than published as
/// a size no slot could be laid out from.
fn video_size_event(width: i32, height: i32) -> Option<PlayerEvent> {
    let width = u32::try_from(width).ok().filter(|w| *w > 0)?;
    let height = u32::try_from(height).ok().filter(|h| *h > 0)?;
    Some(PlayerEvent::VideoSize { width, height })
}

/// Whether a state the host reports through `nativeOnState` is actually new,
/// or a duplicate of what the snapshot already answers.
///
/// The initial Loading is published by [`AndroidBackend::open`] itself
/// (module doc's contract table), so the host's own first callback — Loading
/// most of all, but any repeat is the same kind of no-op — must not be
/// redelivered as a second event a listener would see twice for one real
/// transition.
fn should_publish_state(current: PlaybackState, reported: PlaybackState) -> bool {
    current != reported
}

/// Lock `mutex`, recovering from poisoning rather than propagating it — a
/// panic caught by an export's `catch_unwind` must not turn every later call
/// into one of its own. The guarded data is a plain map of handles, so a
/// poisoned view is still coherent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The [`Shared`] a callback should publish into, cloned **out** of
/// [`SESSIONS`] so the lock is released before the listener runs (module
/// doc's *Session registry*). `None` — an answer for a session this side no
/// longer knows, i.e. a late callback for a closed one — is logged and
/// dropped.
fn session_shared(session: i32, op: &str) -> Option<Arc<Shared>> {
    let shared = lock(&SESSIONS).get(&session).map(Arc::clone);
    if shared.is_none() {
        log::debug!("frust-video-player: {op} for unknown session {session} — dropped");
    }
    shared
}

// --- JNI plumbing -----------------------------------------------------------

/// Run `f` with a live [`Env`] and the cached `FrustVideoPlayerHost` class
/// inside a scoped JNI attachment, flattening the two error layers: a
/// missing platform handle → [`VideoError::PlatformNotInitialized`] (the
/// readiness flag is checked *first*, before any JNI work), a JVM attach
/// failure → [`VideoError::Host`]. `f` itself already returns a
/// [`VideoError`].
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, VideoError>,
) -> Result<T, VideoError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(VideoError::PlatformNotInitialized)
        }
        Err(other) => Err(VideoError::Host(format!(
            "android video backend: platform handle error: {other}"
        ))),
    }
}

/// The cached [`HOST_CLASS`], loading it on first use. A racing loser's
/// reference is dropped immediately (`Global`'s own `Drop` releases it), so
/// at most one global ref survives.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, VideoError> {
    if let Some(class) = HOST_CLASS.get() {
        return Ok(class);
    }
    let class = load_host_class(env, context)?;
    Ok(HOST_CLASS.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass("dev.frust.videoplayer.FrustVideoPlayerHost")`,
/// promoted to a process-lifetime global reference.
///
/// The application `Context`'s classloader is the only loader that can see
/// app-defined classes — a bare `FindClass` on a JNI worker thread sees the
/// bootstrap loader only, which is exactly why this explicit path exists
/// (the `FrustCameraHost` mechanism, `plugins/camera/src/android.rs`).
///
/// A failure here means the class is not there, which is what
/// [`VideoError::PlatformNotInitialized`]'s own doc calls "the native player
/// class could not be resolved" — in practice the plugin's Android Gradle
/// module isn't wired into the app. That variant carries no message, so the
/// JNI detail is logged instead of discarded.
fn load_host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, VideoError> {
    run_jni(env, "loading FrustVideoPlayerHost", |env| {
        let loader = env
            .call_method(
                context,
                jni_str!("getClassLoader"),
                jni_sig!("()Ljava/lang/ClassLoader;"),
                &[],
            )?
            .l()?;
        let name = env.new_string(HOST_CLASS_BINARY)?;
        let class = env
            .call_method(
                &loader,
                jni_str!("loadClass"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                &[JValue::Object(&name)],
            )?
            .l()?;
        let class = env.cast_local::<JClass>(class)?;
        env.new_global_ref(class)
    })
    .map_err(|err| {
        log::error!(
            "frust-video-player: could not load {HOST_CLASS_BINARY} ({err}) — is the plugin's \
             Android Gradle module wired into the app?"
        );
        VideoError::PlatformNotInitialized
    })
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// typed [`VideoError::Host`] naming `op`.
///
/// `jni` returns `Err(Error::JavaException)` and leaves the exception
/// **pending** — undefined behaviour for the next JNI call — so we always
/// check/clear it here before returning, whatever `f` reported (the
/// `frust-camera` `run_jni` shape). The video contract has no exception
/// taxonomy to distinguish: the host reports every outcome it *models* as a
/// return code or a `nativeOnError`, so a throw is a host bug and maps to
/// one variant with the class and message preserved in the string.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, VideoError> {
    let result = f(env);
    if env.exception_check() {
        return Err(take_pending_exception(env, op));
    }
    result.map_err(|err| VideoError::Host(format!("android video backend: {op}: {err}")))
}

/// Extract, **clear**, and describe the pending Java exception. Clears first
/// (mirroring `jni`'s own `exception_catch`) so the subsequent class/message
/// queries run without a pending exception; a defensive final clear covers
/// the unlikely case one of those queries itself throws.
fn take_pending_exception(env: &mut Env<'_>, op: &str) -> VideoError {
    let Some(throwable) = env.exception_occurred() else {
        env.exception_clear();
        return VideoError::Host(format!(
            "android video backend: {op}: JNI reported an exception with no throwable"
        ));
    };
    env.exception_clear();

    let class_name = match env.get_object_class(&throwable) {
        Ok(class) => match class.get_name(env) {
            Ok(name) => name.to_string(),
            Err(_) => UNKNOWN_EXCEPTION_CLASS.to_string(),
        },
        Err(_) => UNKNOWN_EXCEPTION_CLASS.to_string(),
    };
    let message = match throwable.get_message(env) {
        Ok(message) => message.to_string(),
        Err(_) => NO_MESSAGE.to_string(),
    };

    // Defensive: don't leave a second exception pending for the next JNI call.
    if env.exception_check() {
        env.exception_clear();
    }

    VideoError::Host(format!(
        "android video backend: {op}: {class_name}: {message}"
    ))
}

/// Stand-in when a throwable's own class cannot be read.
const UNKNOWN_EXCEPTION_CLASS: &str = "<unknown exception class>";
/// Stand-in when a throwable's — or `nativeOnError`'s — message cannot be
/// read, or was not sent at all.
const NO_MESSAGE: &str = "<no message>";

/// `nativeOnError`'s `message` as a Rust string.
///
/// A null or unreadable message degrades to [`NO_MESSAGE`] rather than
/// abandoning the event: the `code` is the part a caller matches on, and
/// dropping an error entirely because its text could not be read would leave
/// the session stuck in its previous state.
fn error_text(env: &Env<'_>, message: &JString<'_>) -> String {
    if message.is_null() {
        return NO_MESSAGE.to_owned();
    }
    message.try_to_string(env).unwrap_or_else(|err| {
        log::warn!("frust-video-player: nativeOnError carried an unreadable message ({err})");
        NO_MESSAGE.to_owned()
    })
}

// --- Kotlin -> Rust JNI exports (contract table above) ---------------------

/// `Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnState` — the
/// session's lifecycle moved (contract table's `0` Idle … `6` Error).
///
/// A code outside the frozen table is logged and ignored, never guessed at:
/// [`PlaybackState::from_code`] is the one place those codes are decoded.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnState<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    state: jint,
) {
    env.with_env(|_env| {
        log::trace!("frust-video-player: nativeOnState(session={session}, state={state})");
        if let Some(shared) = session_shared(session, "nativeOnState") {
            match PlaybackState::from_code(state) {
                Some(reported) => {
                    let current = shared.snapshot().state;
                    if should_publish_state(current, reported) {
                        shared.publish(PlayerEvent::StateChanged(reported));
                    } else {
                        log::trace!(
                            "frust-video-player: nativeOnState(session={session}) repeated \
                             {reported:?} — dropped as a duplicate"
                        );
                    }
                }
                None => log::warn!(
                    "frust-video-player: nativeOnState(session={session}) reported unknown state \
                     {state} — ignored"
                ),
            }
        }
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnPosition` — the
/// position advanced, carrying the duration as currently known
/// ([`DURATION_UNKNOWN`] while it is not).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnPosition<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    position_ms: jlong,
    duration_ms: jlong,
) {
    env.with_env(|_env| {
        log::trace!(
            "frust-video-player: nativeOnPosition(session={session}, position={position_ms}, \
             duration={duration_ms})"
        );
        if let Some(shared) = session_shared(session, "nativeOnPosition") {
            shared.publish(position_event(position_ms, duration_ms));
        }
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnVideoSize` — the
/// decoded picture's pixel geometry became known, or changed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnVideoSize<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    width: jint,
    height: jint,
) {
    env.with_env(|_env| {
        log::debug!(
            "frust-video-player: nativeOnVideoSize(session={session}, width={width}, \
             height={height})"
        );
        if let Some(shared) = session_shared(session, "nativeOnVideoSize") {
            match video_size_event(width, height) {
                Some(event) => shared.publish(event),
                None => log::debug!(
                    "frust-video-player: nativeOnVideoSize(session={session}) reported \
                     {width}x{height}, which is not a usable size — dropped"
                ),
            }
        }
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnError` —
/// playback failed; `code` is the contract's host-error table and `message`
/// the host's own text.
///
/// Publishing this also moves the session to [`PlaybackState::Error`] and
/// fills [`crate::PlayerSnapshot::error`] — [`Shared::publish`] does both, so
/// a listener and a snapshot reader agree about the failure.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_videoplayer_FrustVideoPlayerHost_nativeOnError<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    code: jint,
    message: JString<'local>,
) {
    env.with_env(|env| {
        if let Some(shared) = session_shared(session, "nativeOnError") {
            let message = error_text(env, &message);
            log::warn!(
                "frust-video-player: nativeOnError(session={session}, code={code}): {message}"
            );
            shared.publish(PlayerEvent::Error(VideoError::from_host_code(
                code, &message,
            )));
        }
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        COMMAND_OK, COMMAND_UNKNOWN_SESSION, DURATION_UNKNOWN, SOURCE_KIND_ASSET, SOURCE_KIND_FILE,
        SOURCE_KIND_URL, command_result, open_error, position_arg, position_event,
        should_publish_state, source_args, url_scheme, video_size_event,
    };
    use crate::{PlaybackState, PlayerEvent, VideoError, VideoFit, VideoSource};

    /// Each source maps to its contract `sourceKind`, with the string the
    /// host resolves left exactly as the caller wrote it.
    #[test]
    fn source_args_maps_each_kind() {
        assert_eq!(
            source_args(&VideoSource::File(PathBuf::from("/sdcard/clip.mp4"))),
            Ok((SOURCE_KIND_FILE, "/sdcard/clip.mp4"))
        );
        assert_eq!(
            source_args(&VideoSource::Asset("media/intro.mp4".to_owned())),
            Ok((SOURCE_KIND_ASSET, "media/intro.mp4"))
        );
        assert_eq!(
            source_args(&VideoSource::Url("https://example.test/a.m3u8".to_owned())),
            Ok((SOURCE_KIND_URL, "https://example.test/a.m3u8"))
        );
    }

    /// A relative file path is refused here rather than handed to a host
    /// that would resolve it against an unrelated working directory.
    #[test]
    fn source_args_refuses_a_relative_file_path() {
        let source = VideoSource::File(PathBuf::from("clip.mp4"));
        let refused = source_args(&source);
        assert!(
            matches!(refused, Err(VideoError::UnsupportedSource(_))),
            "expected UnsupportedSource, got {refused:?}"
        );
    }

    /// An asset name is *not* a path, so a relative one is exactly right.
    #[test]
    fn source_args_accepts_a_relative_asset_name() {
        assert_eq!(
            source_args(&VideoSource::Asset("intro.mp4".to_owned())),
            Ok((SOURCE_KIND_ASSET, "intro.mp4"))
        );
    }

    /// `http`/`https` are the only accepted URL schemes, matched
    /// ASCII-case-insensitively.
    #[test]
    fn source_args_accepts_only_http_and_https_urls() {
        assert_eq!(
            source_args(&VideoSource::Url("http://example.test/a.mp4".to_owned())),
            Ok((SOURCE_KIND_URL, "http://example.test/a.mp4"))
        );
        assert_eq!(
            source_args(&VideoSource::Url("HTTP://example.test/a.mp4".to_owned())),
            Ok((SOURCE_KIND_URL, "HTTP://example.test/a.mp4"))
        );
        assert_eq!(
            source_args(&VideoSource::Url("https://example.test/a.mp4".to_owned())),
            Ok((SOURCE_KIND_URL, "https://example.test/a.mp4"))
        );
    }

    /// A scheme outside the allowlist is refused, and the message names only
    /// the scheme, never the full (potentially sensitive) URL.
    #[test]
    fn source_args_refuses_a_disallowed_url_scheme() {
        for url in ["file:///x", "content://x", "data:text/plain,x", "rtmp://x"] {
            let source = VideoSource::Url(url.to_owned());
            let refused = source_args(&source);
            match refused {
                Err(VideoError::UnsupportedSource(message)) => {
                    assert!(
                        !message.contains(url),
                        "message `{message}` echoed the refused URL `{url}`"
                    );
                }
                other => panic!("expected UnsupportedSource for `{url}`, got {other:?}"),
            }
        }
    }

    /// A URL string with no scheme at all is refused too, not accepted or
    /// mishandled.
    #[test]
    fn source_args_refuses_a_schemeless_url() {
        let source = VideoSource::Url("example.test/a.mp4".to_owned());
        let refused = source_args(&source);
        assert!(
            matches!(refused, Err(VideoError::UnsupportedSource(_))),
            "expected UnsupportedSource, got {refused:?}"
        );
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

    /// `openPlayer`'s negative returns decode through the crate-wide host
    /// table, with the refused source named in the message.
    #[test]
    fn open_error_decodes_the_host_table() {
        assert_eq!(
            open_error(-3, SOURCE_KIND_URL, "https://example.test/a.mp4"),
            VideoError::PlatformNotInitialized
        );
        assert_eq!(
            open_error(-1, SOURCE_KIND_FILE, "/a.mp4"),
            VideoError::UnknownSession
        );

        let VideoError::UnsupportedSource(message) = open_error(-2, SOURCE_KIND_ASSET, "a.mp4")
        else {
            panic!("code -2 must decode to UnsupportedSource");
        };
        assert!(message.contains("asset"), "{message}");
        assert!(message.contains("a.mp4"), "{message}");

        assert!(matches!(
            open_error(-4, SOURCE_KIND_URL, "https://example.test/a.mp4"),
            VideoError::Network(_)
        ));
        assert!(matches!(
            open_error(-5, SOURCE_KIND_FILE, "/a.mp4"),
            VideoError::Decoder(_)
        ));
        // A host newer than this crate degrades to a readable message.
        assert!(matches!(
            open_error(-99, SOURCE_KIND_FILE, "/a.mp4"),
            VideoError::Host(_)
        ));
    }

    /// A command's return code maps to the contract's two outcomes, and an
    /// unknown code reports rather than panicking.
    #[test]
    fn command_result_maps_the_contract_codes() {
        assert_eq!(
            command_result("FrustVideoPlayerHost.play", COMMAND_OK),
            Ok(())
        );
        assert_eq!(
            command_result("FrustVideoPlayerHost.play", COMMAND_UNKNOWN_SESSION),
            Err(VideoError::UnknownSession)
        );
        let VideoError::Host(message) = command_result("FrustVideoPlayerHost.play", 7)
            .expect_err("an off-contract code must not report success")
        else {
            panic!("an unknown code must decode to Host");
        };
        assert!(message.contains("FrustVideoPlayerHost.play"), "{message}");
        assert!(message.contains('7'), "{message}");
    }

    /// A seek position saturates instead of wrapping into a small one.
    #[test]
    fn position_arg_saturates() {
        assert_eq!(position_arg(std::time::Duration::from_millis(1_500)), 1_500);
        assert_eq!(position_arg(std::time::Duration::MAX), i64::MAX);
    }

    /// An unknown duration is `None`, not a zero duration.
    #[test]
    fn position_event_reads_an_unknown_duration_as_none() {
        let PlayerEvent::Position { position, duration } = position_event(250, DURATION_UNKNOWN)
        else {
            panic!("expected a Position event");
        };
        assert_eq!(position, std::time::Duration::from_millis(250));
        assert_eq!(duration, None);

        let PlayerEvent::Position { duration, .. } = position_event(250, 9_000) else {
            panic!("expected a Position event");
        };
        assert_eq!(duration, Some(std::time::Duration::from_millis(9_000)));
    }

    /// A non-positive geometry is dropped rather than published as a size no
    /// slot could be laid out from.
    #[test]
    fn video_size_event_drops_a_non_positive_size() {
        assert_eq!(
            video_size_event(1920, 1080),
            Some(PlayerEvent::VideoSize {
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(video_size_event(0, 1080), None);
        assert_eq!(video_size_event(1920, -1), None);
    }

    /// The platform-view spellings the Kotlin factory parses, asserted from
    /// the module that owns the rest of the Android contract even though the
    /// literals themselves live at the crate root (module doc).
    #[test]
    fn android_platform_view_spellings_are_frozen() {
        assert_eq!(
            crate::VIEW_TYPE,
            "dev.frust.videoplayer.VideoPlayerViewFactory"
        );
        assert_eq!(
            crate::params_json_with(crate::SESSION_KEY, 7, VideoFit::Contain),
            r#"{"sessionId":7,"fit":"contain"}"#
        );
        assert_eq!(
            crate::params_json_with(crate::SESSION_KEY, 7, VideoFit::Cover),
            r#"{"sessionId":7,"fit":"cover"}"#
        );
    }

    /// A state equal to what the snapshot already reports — Loading most of
    /// all, the one `open` publishes itself before the host's own callback
    /// can land — is a duplicate and must not be republished.
    #[test]
    fn should_publish_state_drops_a_repeat_of_the_current_state() {
        assert!(!should_publish_state(
            PlaybackState::Loading,
            PlaybackState::Loading
        ));
        assert!(!should_publish_state(
            PlaybackState::Playing,
            PlaybackState::Playing
        ));
    }

    /// An actual transition is always published, whatever the two states are.
    #[test]
    fn should_publish_state_admits_a_real_transition() {
        assert!(should_publish_state(
            PlaybackState::Loading,
            PlaybackState::Paused
        ));
        assert!(should_publish_state(
            PlaybackState::Playing,
            PlaybackState::Buffering
        ));
        assert!(should_publish_state(
            PlaybackState::Idle,
            PlaybackState::Loading
        ));
    }

    /// When close runs on the main thread, the host's inline teardown callback
    /// already moves the snapshot to Idle, so the post-removal publish must
    /// not repeat it. This test guards the pure decision: a close after the
    /// host already reported Idle publishes nothing more.
    #[test]
    fn a_close_after_the_host_already_reported_idle_publishes_nothing_more() {
        assert!(!should_publish_state(
            PlaybackState::Idle,
            PlaybackState::Idle
        ));
        assert!(should_publish_state(
            PlaybackState::Playing,
            PlaybackState::Idle
        ));
    }
}
