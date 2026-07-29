//! Camera page: live Mode B preview behind frust
//! chrome, permission flow, still capture, and an image-stream readout — the
//! **device-gate vehicle for the whole `frust-camera` feature** (on-device
//! proof is a separate, human-run gate — see `docs/DEVELOPMENT.md`'s Camera
//! manual test; this page only owns wiring + compile/gate correctness).
//!
//! # Mode B slot, reusing the platform-views test bed
//!
//! [`crate::home_page`] already wraps every section's content in a
//! `scroll_view` and paints an [`crate::AppBackground`] base layer
//! under the whole app (the Mode B host's required opaque app-root
//! background, `docs/CODE_STANDARDS.md`'s Platform-View Conventions) — both
//! reused here unchanged rather than re-derived. This page's own
//! [`preview_block`] is therefore already inside the shared scroll view:
//! scrolling it off/on-screen exercises the platform view's real
//! dispose→create cycle — the camera session itself lives independent of
//! the view and outlives it, exactly like `platform_views.rs`'s Mode B slot
//! does.
//!
//! # Flow
//!
//! Page mount → [`frust::use_task`] + [`frust::spawn_blocking`] runs
//! [`Camera::request_permission`] once (this crate's own `on_cleanup`
//! rule: registered under [`CameraPage`]'s [`Component`] owner). Denied
//! shows the typed error state; [`PermissionStatus::Granted`] opens a
//! [`CameraSession`] and the preview slot appears. [`PermissionStatus::NeedsUi`]
//! is Android-specific and documented on the type itself; the recorded
//! escalation if a manual retry doesn't resolve it on device is a
//! transparent proxy Activity — a decision for the on-device gate, not this
//! page's.
//!
//! # Session lifecycle
//!
//! The opened [`CameraSession`] lives in [`CameraPageState::session_cell`]
//! (an `Arc<Mutex<..>>`, not an `Rc`, only because [`frust::on_cleanup`]
//! requires a `Send + Sync` closure — mirrors `frust-reactive::task`'s own
//! `Coordinator::bg_abort` field). [`CameraPage::init`] registers the one
//! `on_cleanup` that closes it; teardown (leaving this tab) disposes the
//! component's owner and runs it — **never a `Drop` impl**
//! (`docs/CODE_STANDARDS.md`'s State & Reactivity Conventions).
//!
//! # Image-stream readout
//!
//! [`CameraSession::start_image_stream`]'s callback runs on a plugin-owned
//! thread and must never write a signal directly
//! (`plugins/camera`'s own module docs): this page hands
//! frames off through a plain `Arc<`[`StreamStats`]`>` of atomics instead of
//! any reactive primitive (see [`start_stream`], which wires the real
//! callback), and reads it back from [`CameraPage::build`] while a
//! [`FrameTicker`] (the same escape hatch `interactions.rs` documents) keeps
//! that rebuild running once per frame. The stream is real and wired on both
//! shipped backends — the device gate measured it delivering frames at
//! ~29 fps on Android and ~22 fps on iOS through this exact path; this
//! page's readout is the live proof of that, not a stub.
//! [`ImageFormat::Bgra`] stays Apple-only (see that variant's doc), so this
//! page always requests [`ImageFormat::Yuv420`], the cross-platform format
//! both backends deliver.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use frust::{
    AnyView, AsyncValue, Axis, ButtonStyle, Color, Component, EdgeInsets, FlexChild, FlexView, Get,
    Padding, PlatformViewView, SizedBox, Theme, UseTask, any, button, component, inflexible,
    on_cleanup, platform_view, spawn_blocking, text, use_context, use_task,
};
use frust_camera::{
    Camera, CameraError, CameraSession, ImageFormat, Lens, PermissionStatus, Resolution,
};

// Low-level escape hatch (mirrors `interactions.rs`'s `FrameTicker` /
// `appbar.rs`'s `AnchorReporter` — this crate's `Cargo.toml` already carries
// `frust-core`/`kurbo` as real dependencies for those uses): only
// [`FrameTicker`] below reaches for this; every other widget on this page
// comes from the `frust` facade.
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::Size;

use crate::CatalogState;

/// The live preview slot's fixed size. `CameraSession::preview_aspect_ratio()`
/// stays `0.0` until the platform reports its first frame geometry (the
/// crate's own doc), so an aspect-derived size isn't available yet — a fixed
/// box is the honest v1 rather than guessing an aspect.
const PREVIEW_W: f64 = 320.0;
const PREVIEW_H: f64 = 240.0;

/// See the page-fn contract in [`crate::pages`]. Camera has no shared signal
/// to read — it only mounts [`CameraPage`]'s own retained local state (the
/// [module docs](self)'s Session lifecycle section).
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(component(CameraPage))
}

// ---------------------------------------------------------------------------
// Retained state + Component
// ---------------------------------------------------------------------------

/// Shared, thread-safe counters the image-stream callback ([`start_stream`])
/// writes into — see the [module docs](self)'s Image-stream readout section.
/// `Ordering::Relaxed` throughout: these are display-only counters with no
/// synchronization requirement beyond "eventually visible" to the next
/// UI-thread rebuild.
#[derive(Default)]
struct StreamStats {
    /// Total frames delivered since the current stream started.
    frames: AtomicU64,
    /// The first byte of the first plane of the most recent frame — a cheap
    /// delivery proxy, **not** a real luminance sample (BGRA's first byte is
    /// blue, YUV420's is a Y sample; either way it only proves bytes moved).
    last_byte: AtomicU8,
    /// Whether at least one frame has arrived since the stream started.
    has_frame: AtomicBool,
}

/// [`CameraPage`]'s retained local state. See the [module docs](self).
struct CameraPageState {
    /// The permission fetch — `Ready(Granted)` gates opening a session (see
    /// [`CameraPage::build`]).
    permission: UseTask<PermissionStatus>,
    /// The open session, if any. `Arc<Mutex<..>>` (not `Rc`) only because
    /// [`on_cleanup`] requires a `Send + Sync` closure — see the
    /// [module docs](self). The held value is itself `Arc`-wrapped so
    /// [`take_picture_handler`]'s background fetch can cheaply *clone* a
    /// handle out under a brief lock and release it before making the
    /// blocking `take_picture` call — never holding this mutex across that
    /// call (root cause A's consumer half, this module's own Details point
    /// 3). `CameraSession` itself has no public `Clone` (its backend field
    /// is crate-private in `frust-camera`), so cloning the wrapper `Arc` is
    /// the only way to get an owned, `'static` handle into `spawn_blocking`.
    session_cell: Arc<Mutex<Option<Arc<CameraSession>>>>,
    /// The lens the current (or next-to-open) session targets — defaults to
    /// [`Lens::Back`], [`Camera::open`]'s original hardcoded value.
    /// [`switch_lens_handler`] flips this and clears [`Self::open_attempted`]
    /// so [`CameraPage::build`]'s existing open branch reopens on the new
    /// lens next rebuild, rather than adding a second open path (see the
    /// [module docs](self)).
    lens: Lens,
    /// Set once [`Camera::open`] has been attempted for the current
    /// permission grant, so a failed open doesn't retry every rebuild.
    open_attempted: bool,
    /// [`Camera::open`]'s error, if the attempt above failed.
    open_error: Option<String>,
    /// The still-capture task: `Ready(Some(path))` echoes the requested
    /// path (capture completion itself is asynchronous and unobservable
    /// from this v1 API — see [`take_picture_handler`]); `Ready(None)`
    /// covers the (UI-unreachable, since [`capture_block`] only mounts
    /// once a session exists) no-session case, including this task's own
    /// automatic first fetch at page mount, before any session is open —
    /// deliberately not an [`AsyncValue::Error`], so it never flashes a
    /// spurious failure message on load.
    capture: UseTask<Option<String>>,
    /// Whether an image stream is currently (believed) running.
    streaming: bool,
    /// The last `start_image_stream`/`stop_image_stream` error, if any.
    stream_error: Option<String>,
    /// When the current stream started, for the frames/sec readout.
    stream_started_at: Option<Instant>,
    /// The stream's shared counters — see [`StreamStats`].
    stream_stats: Arc<StreamStats>,
}

/// The camera page's own [`Component`] (see the [module docs](self)).
struct CameraPage;

impl Component for CameraPage {
    type State = CameraPageState;

    fn init(&self) -> CameraPageState {
        // `Camera::request_permission` blocks on the platform permission
        // machinery — pair with `spawn_blocking`, never call on the UI
        // thread (the crate's own "Blocking API" doc, `docs/CODE_STANDARDS.md`'s
        // heavy-work routing rule). Flattened to `Result<PermissionStatus,
        // CameraError>` so `use_task`'s single-`Result` `Fut` bound is met —
        // a `spawn_blocking` join failure (panic) reports as a
        // `CameraError::Platform` rather than a second error type.
        let permission = use_task(|| async {
            match spawn_blocking(Camera::request_permission).await {
                Ok(result) => result,
                Err(join_err) => Err(CameraError::Platform(join_err.to_string())),
            }
        });

        let session_cell: Arc<Mutex<Option<Arc<CameraSession>>>> = Arc::new(Mutex::new(None));
        {
            let cell = session_cell.clone();
            on_cleanup(move || {
                if let Some(session) = cell.lock().expect("session cell poisoned").take() {
                    session.close();
                }
            });
        }

        // `take_picture` blocks up to 15s (Android) / 10s (Apple) — same
        // "pair with `spawn_blocking`, never call on the UI thread" contract
        // as `request_permission` above (f1 documents this on the crate
        // side). Extracting the session is a brief-lock `Arc` *clone* (see
        // `session_cell`'s doc comment), never a lock held across the
        // blocking call itself. `T = Option<String>`: `None` covers both
        // "no active session" and this task's own automatic first fetch at
        // page mount (before `Camera::open` has run) without surfacing a
        // spurious error — see `capture`'s doc comment.
        let capture = {
            let cell = session_cell.clone();
            use_task(move || {
                let cell = cell.clone();
                async move {
                    let session = cell.lock().expect("session cell poisoned").clone();
                    let Some(session) = session else {
                        return Ok(None);
                    };
                    let path = capture_path();
                    match spawn_blocking(move || {
                        let display = path.display().to_string();
                        session.take_picture(&path).map(|()| display)
                    })
                    .await
                    {
                        Ok(Ok(display)) => Ok(Some(display)),
                        Ok(Err(err)) => Err(err),
                        Err(join_err) => Err(CameraError::Platform(join_err.to_string())),
                    }
                }
            })
        };

        CameraPageState {
            permission,
            session_cell,
            lens: Lens::Back,
            open_attempted: false,
            open_error: None,
            capture,
            streaming: false,
            stream_error: None,
            stream_started_at: None,
            stream_stats: Arc::new(StreamStats::default()),
        }
    }

    fn build(&self, state: &mut CameraPageState) -> AnyView<CameraPageState> {
        let permission = state.permission.signal().get();

        // Open the session exactly once per grant (idempotent via
        // `open_attempted`) — see the [module docs](self)'s Flow section.
        if matches!(&permission, AsyncValue::Ready(PermissionStatus::Granted))
            && !state.open_attempted
        {
            state.open_attempted = true;
            match Camera::open(state.lens, Resolution::Auto) {
                Ok(session) => {
                    *state.session_cell.lock().expect("session cell poisoned") =
                        Some(Arc::new(session));
                }
                Err(err) => state.open_error = Some(err.to_string()),
            }
        }

        let has_session = state
            .session_cell
            .lock()
            .expect("session cell poisoned")
            .is_some();

        let mut children: Vec<FlexChild<CameraPageState>> = vec![
            block(vec![
                inflexible(label("Camera")),
                gap(6.0),
                inflexible(caption(
                    "Live Mode B preview behind frust chrome \u{2014} permission, still \
                     capture, and a stream readout. Scroll this slot off/on-screen to \
                     exercise the platform view's real dispose/revive cycle.",
                )),
            ]),
            permission_block(&permission, state.open_error.as_deref()),
            lens_block(state, has_session),
        ];

        if has_session {
            children.push(preview_block(state));
            children.push(capture_block(state));
            children.push(stream_block(state));
            if state.streaming {
                // Keeps this Component's `build` re-invoked every frame while
                // a stream is (believed) running, so the frames/sec readout
                // stays live — see the [module docs](self)'s readout section.
                children.push(frame_ticker());
            }
        }

        children.push(gap(12.0));
        children.push(inflexible(filler_rows()));

        any(Padding(
            EdgeInsets::all(16.0),
            FlexView::new(Axis::Vertical, children),
        ))
    }
}

// ---------------------------------------------------------------------------
// Section chrome (mirrors appbar.rs/interactions.rs/motion.rs/platform_views.rs)
// ---------------------------------------------------------------------------

/// Live-theme accent-text role (`primary`), falling back to the Glyph
/// baseline pre-context.
fn amber() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .on_surface_variant
}

/// An error-state ink.
fn error_ink() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .error
}

/// A demo heading in accent amber.
fn label(s: impl Into<String>) -> AnyView<CameraPageState> {
    any(text(s).size(13.0).color(amber()))
}

/// A muted per-demo caption.
fn caption(s: impl Into<String>) -> AnyView<CameraPageState> {
    any(text(s).size(11.0).color(muted()))
}

/// A fixed-height vertical spacer between demo blocks.
fn gap(h: f64) -> FlexChild<CameraPageState> {
    inflexible(SizedBox(None, Some(h)))
}

/// Wrap a demo's rows in a padded vertical column (one showcase block).
fn block(children: Vec<FlexChild<CameraPageState>>) -> FlexChild<CameraPageState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

/// A bounded list of filler rows so the preview slot can scroll fully
/// off-screen and back (task 11's Details point 4; the M1 device-viewport
/// lesson `platform_views.rs`'s own `filler_rows` doc comment tells in
/// full) — duplicated locally per that module's own precedent (private,
/// per-module, not shared).
fn filler_rows() -> AnyView<CameraPageState> {
    let rows: Vec<AnyView<CameraPageState>> = (1..=64)
        .map(|i| {
            any(Padding(
                EdgeInsets::symmetric(0.0, 8.0),
                text(format!("row {i:02} \u{2014} camera page filler")).size(12.0),
            ))
        })
        .collect();
    any(FlexView::new(
        Axis::Vertical,
        rows.into_iter().map(inflexible).collect(),
    ))
}

// ---------------------------------------------------------------------------
// Permission block
// ---------------------------------------------------------------------------

fn permission_block(
    permission: &AsyncValue<PermissionStatus>,
    open_error: Option<&str>,
) -> FlexChild<CameraPageState> {
    let (message, is_problem) = match permission {
        AsyncValue::Idle | AsyncValue::Loading(_) => {
            ("Requesting camera permission\u{2026}".to_string(), false)
        }
        AsyncValue::Ready(PermissionStatus::Granted) => match open_error {
            Some(err) => (
                format!("Permission granted, but opening the camera failed: {err}"),
                true,
            ),
            None => ("Permission granted.".to_string(), false),
        },
        AsyncValue::Ready(PermissionStatus::Denied) => (
            "Camera permission denied \u{2014} enable it in system settings, then retry."
                .to_string(),
            true,
        ),
        // Android-specific: no cached Activity yet for the system dialog.
        // The recorded escalation if a manual retry doesn't resolve this on
        // device is a transparent proxy-Activity.
        AsyncValue::Ready(PermissionStatus::NeedsUi) => (
            "No foreground activity cached yet for the permission dialog \u{2014} retry once \
             the app is in the foreground (PLAN.md's permission-plumbing note)."
                .to_string(),
            true,
        ),
        AsyncValue::Ready(PermissionStatus::Pending) => (
            "System permission dialog is showing\u{2026}".to_string(),
            false,
        ),
        // `PermissionStatus` is `#[non_exhaustive]` — a future variant reads
        // as an unspecified problem state rather than a compile break.
        AsyncValue::Ready(_) => ("Unknown permission state.".to_string(), true),
        AsyncValue::Error(err) => (format!("Permission request failed: {err}"), true),
    };

    let color = if is_problem { error_ink() } else { muted() };
    block(vec![
        inflexible(label("Permission")),
        gap(6.0),
        inflexible(any(text(message).size(12.0).color(color))),
        gap(6.0),
        inflexible(any(button("Retry permission", retry_permission_handler)
            .style(ButtonStyle::Secondary)
            .small())),
    ])
}

fn retry_permission_handler(state: &mut CameraPageState) {
    state.open_attempted = false;
    state.open_error = None;
    state.permission.restart();
}

// ---------------------------------------------------------------------------
// Lens switch (C5)
// ---------------------------------------------------------------------------

/// Close the current session (if any) and flip [`CameraPageState::lens`],
/// then clear [`CameraPageState::open_attempted`] so [`CameraPage::build`]'s
/// existing open branch — not a second one — reopens on the new lens next
/// rebuild. A no-op while no session is open (guarded by the early
/// `let...else` below), matching this control's "disabled (or a no-op)
/// while no session exists" contract (this task's Acceptance).
///
/// `Camera::open` isn't itself one of the crate's two blocking calls (only
/// [`Camera::request_permission`]/[`CameraSession::take_picture`] are — the
/// crate's own Blocking API table); [`CameraPage::build`]'s open branch
/// already calls it directly on the UI thread for the very first open, so
/// reopening the same way here adds no second blocking call.
fn switch_lens_handler(state: &mut CameraPageState) {
    let outgoing = state
        .session_cell
        .lock()
        .expect("session cell poisoned")
        .take();
    let Some(session) = outgoing else {
        // No session yet — nothing to switch.
        return;
    };
    session.close();

    state.lens = match state.lens {
        Lens::Front => Lens::Back,
        // `Lens` is `#[non_exhaustive]`; `Back` and any future variant both
        // just park on `Front` rather than failing to compile.
        _ => Lens::Front,
    };
    state.open_attempted = false;
    state.open_error = None;

    // Capture/stream state belongs to the outgoing session — reset both so
    // nothing from it leaks into the new one (this task's Acceptance).
    state.streaming = false;
    state.stream_started_at = None;
    state.stream_error = None;
    state.stream_stats.frames.store(0, Ordering::Relaxed);
    state.stream_stats.has_frame.store(false, Ordering::Relaxed);
    state.stream_stats.last_byte.store(0, Ordering::Relaxed);
    // Re-runs the capture task against the now-empty `session_cell`,
    // resolving to `Ready(None)` ("No capture requested yet.") rather than
    // showing a stale path/error from the outgoing session — the same
    // `restart` idiom [`take_picture_handler`] uses, not a new mechanism.
    state.capture.restart();
}

fn lens_block(state: &CameraPageState, has_session: bool) -> FlexChild<CameraPageState> {
    let switch_label = match state.lens {
        Lens::Back => "Switch to front camera",
        Lens::Front => "Switch to back camera",
        _ => "Switch camera",
    };
    let current = match state.lens {
        Lens::Back => "back",
        Lens::Front => "front",
        _ => "unknown",
    };

    block(vec![
        inflexible(label("Lens")),
        gap(6.0),
        inflexible(caption(format!("Current: {current}."))),
        gap(6.0),
        inflexible(any(button(switch_label, switch_lens_handler)
            .style(ButtonStyle::Secondary)
            .small())),
        gap(6.0),
        inflexible(caption(if has_session {
            "Closes the current session and reopens it on the other lens."
        } else {
            "No active session yet \u{2014} a no-op until one opens."
        })),
    ])
}

// ---------------------------------------------------------------------------
// Preview slot (Mode B)
// ---------------------------------------------------------------------------

/// Apply [`PlatformViewView::debug_fill`] only in a debug build (see
/// `platform_views.rs`'s own `maybe_debug_fill` precedent: the method
/// itself doesn't exist under a release profile, so a plain conditional
/// reassignment would leave an "unused `mut`" lint in release builds).
fn maybe_debug_fill(view: PlatformViewView) -> PlatformViewView {
    #[cfg(debug_assertions)]
    {
        view.debug_fill()
    }
    #[cfg(not(debug_assertions))]
    {
        view
    }
}

fn preview_block(state: &CameraPageState) -> FlexChild<CameraPageState> {
    let guard = state.session_cell.lock().expect("session cell poisoned");
    let Some(session) = guard.as_ref() else {
        drop(guard);
        return block(vec![inflexible(caption("No active session."))]);
    };

    // `view_type` is target-gated by `CameraSession` itself (Android returns
    // a fully-qualified class name, iOS a bare runtime name —
    // `docs/CODE_STANDARDS.md`'s platform-view factory `viewType` LAW).
    let view_type = session.preview_view_type().to_string();
    // The real backend-minted session id (`{"sessionId":N}` on Android,
    // `{"session":N}` on Apple) — not a page-local counter, which drifts
    // from Android's zero-based ids (root cause D).
    let params = session.params_json();
    drop(guard);

    let slot = maybe_debug_fill(
        platform_view(view_type)
            .size(PREVIEW_W, PREVIEW_H)
            .params_json(params)
            .semantics_label("Live camera preview"),
    );

    block(vec![
        inflexible(label("Preview")),
        gap(6.0),
        inflexible(any(slot)),
    ])
}

// ---------------------------------------------------------------------------
// Still capture
// ---------------------------------------------------------------------------

/// A timestamped capture path under the OS temp directory — plain, no
/// `frust-paths` dependency added for this device-gate page (out of this
/// task's file scope).
fn capture_path() -> PathBuf {
    let epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("glyphcatalog-capture-{epoch_ms}.jpg"))
}

/// `take_picture` blocks up to 15s (Android) / 10s (Apple) — this only
/// re-triggers the [`CameraPageState::capture`] task built in
/// [`CameraPage::init`] (the same `spawn_blocking`-off-the-UI-thread
/// pattern [`retry_permission_handler`] already uses for `permission`);
/// the mutex is never held across the blocking call itself (see
/// `session_cell`'s doc comment).
fn take_picture_handler(state: &mut CameraPageState) {
    state.capture.restart();
}

fn capture_block(state: &CameraPageState) -> FlexChild<CameraPageState> {
    let capture = state.capture.signal().get();
    let status = match &capture {
        AsyncValue::Idle | AsyncValue::Ready(None) => "No capture requested yet.".to_string(),
        AsyncValue::Loading(_) => "Capturing\u{2026}".to_string(),
        // Capture completion is asynchronous and unobservable from this v1
        // API (module docs) — this echoes the requested path, not a
        // confirmed-written one.
        AsyncValue::Ready(Some(path)) => format!("Capture requested \u{2192} {path}"),
        AsyncValue::Error(err) => format!("Capture failed: {err}"),
    };

    block(vec![
        inflexible(label("Still capture")),
        gap(6.0),
        inflexible(any(button("Take picture", take_picture_handler)
            .style(ButtonStyle::Primary)
            .small())),
        gap(6.0),
        inflexible(any(text(status).size(11.0).color(muted()))),
    ])
}

// ---------------------------------------------------------------------------
// Image stream
// ---------------------------------------------------------------------------

/// Wires [`CameraSession::start_image_stream`]'s callback to plain atomics
/// only — never a signal write off the UI thread (module docs).
///
/// Requests [`ImageFormat::Yuv420`], the cross-platform choice both `lib.rs`
/// and `android.rs` document: CameraX only ever delivers RGBA_8888, so the
/// Android backend refuses [`ImageFormat::Bgra`] with a typed error (root
/// cause E) — no `cfg(target_os)` branching needed since the callback below
/// is format-agnostic.
fn start_stream(session: &CameraSession, stats: Arc<StreamStats>) -> Result<(), CameraError> {
    session.start_image_stream(ImageFormat::Yuv420, move |frame| {
        stats.frames.fetch_add(1, Ordering::Relaxed);
        if let Some(plane) = frame.planes.first()
            && let Some(&byte) = plane.data.first()
        {
            stats.last_byte.store(byte, Ordering::Relaxed);
            stats.has_frame.store(true, Ordering::Relaxed);
        }
    })
}

fn toggle_stream_handler(state: &mut CameraPageState) {
    if state.streaming {
        {
            let guard = state.session_cell.lock().expect("session cell poisoned");
            if let Some(session) = guard.as_ref() {
                session.stop_image_stream();
            }
        }
        state.streaming = false;
        state.stream_started_at = None;
        return;
    }

    let stats = state.stream_stats.clone();
    let result = {
        let guard = state.session_cell.lock().expect("session cell poisoned");
        guard.as_ref().map(|session| start_stream(session, stats))
    };
    match result {
        Some(Ok(())) => {
            state.stream_stats.frames.store(0, Ordering::Relaxed);
            state.stream_stats.has_frame.store(false, Ordering::Relaxed);
            state.streaming = true;
            state.stream_started_at = Some(Instant::now());
            state.stream_error = None;
        }
        Some(Err(err)) => state.stream_error = Some(err.to_string()),
        None => state.stream_error = Some("no active camera session".to_string()),
    }
}

fn stream_block(state: &CameraPageState) -> FlexChild<CameraPageState> {
    let button_label = if state.streaming {
        "Stop stream"
    } else {
        "Start stream"
    };

    let frames = state.stream_stats.frames.load(Ordering::Relaxed);
    let has_frame = state.stream_stats.has_frame.load(Ordering::Relaxed);
    let last_byte = state.stream_stats.last_byte.load(Ordering::Relaxed);
    let fps = state
        .stream_started_at
        .map(|start| {
            let secs = start.elapsed().as_secs_f64();
            if secs > 0.0 {
                frames as f64 / secs
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);

    let readout = if state.streaming {
        if has_frame {
            format!(
                "{frames} frames \u{b7} ~{fps:.1} fps \u{b7} last byte {last_byte} (a \
                 delivery proof, not a real luma sample)"
            )
        } else {
            "streaming \u{2014} waiting for the first frame\u{2026}".to_string()
        }
    } else if let Some(err) = &state.stream_error {
        format!("stream error: {err}")
    } else {
        "stream stopped.".to_string()
    };

    block(vec![
        inflexible(label("Image stream")),
        gap(6.0),
        inflexible(any(button(button_label, toggle_stream_handler)
            .style(ButtonStyle::Secondary)
            .small())),
        gap(6.0),
        inflexible(any(text(readout).size(11.0).color(muted()))),
    ])
}

// ---------------------------------------------------------------------------
// FrameTicker — see `interactions.rs`'s twin for the full rationale.
// ---------------------------------------------------------------------------

/// A zero-size sentinel [`View`]/[`Widget`] pair whose only job is an
/// unconditional [`PaintCtx::request_frame`] call in its own `paint` —
/// mounted only while [`CameraPageState::streaming`] is true, so the
/// frames/sec readout in [`stream_block`] keeps refreshing (the
/// [module docs](self)'s Image-stream readout section).
struct FrameTicker;

impl<State: 'static> View<State> for FrameTicker {
    type Element = FrameTickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FrameTickerWidget {
        FrameTickerWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut FrameTickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::PAINT
    }
}

/// The retained widget for a [`FrameTicker`]. See its doc comment.
struct FrameTickerWidget;

impl Widget for FrameTickerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame();
    }
}

fn frame_ticker() -> FlexChild<CameraPageState> {
    inflexible(FrameTicker)
}
