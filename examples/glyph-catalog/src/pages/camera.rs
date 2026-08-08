//! Camera page: live Mode B preview behind frust
//! chrome, permission flow, still capture, and an image-stream readout — the
//! **device-gate vehicle for the whole `frust-camera` feature** (on-device
//! proof is a separate, human-run gate — see `docs/PLUGINS_DEVELOPMENT.md`'s
//! Camera manual test; this page only owns wiring + compile/gate correctness).
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
//! [`FrameTicker`] (the same hand-rolled widget `interactions.rs` documents) keeps
//! that rebuild running once per frame. The stream is real and wired on both
//! shipped backends — the device gate measured it delivering frames at
//! ~29 fps on Android and ~22 fps on iOS through this exact path; this
//! page's readout is the live proof of that, not a stub.
//! [`ImageFormat::Bgra`] stays Apple-only (see that variant's doc), so this
//! page always requests [`ImageFormat::Yuv420`], the cross-platform format
//! both backends deliver.
//!
//! # Scan strip (measurement gate rig)
//!
//! [`scan_block`] adds a two-mode strip below the readout above, exercising
//! `plugins/camera`'s barcode API (`plugins/camera/README.md` §4 "Barcode
//! stream") end to end. [`ScanMode::Policy`] calls
//! [`CameraSession::start_barcode_stream`] with
//! [`DetectionPolicy::NoDuplicates`] and shows the last detection's
//! (truncated) `raw_value`, its [`BarcodeFormat`], and a detection counter —
//! the muxr consumer shape. [`ScanMode::Timing`] instead calls the raw
//! [`CameraSession::start_image_stream`] and times
//! [`frust_camera::barcode::decode_frame`] by hand with
//! [`std::time::Instant`] inside the callback, keeping a small ring buffer
//! ([`TimingStats::durations_us`]) to surface last/median/p95 decode ms,
//! effective attempts/sec, and the frame resolution — the numbers a
//! device measurement session transcribes into `MEASUREMENTS.md`.
//!
//! Both modes hand results across the callback's plugin thread the same
//! atomics/`Mutex` way [`StreamStats`] already does above — never a signal
//! write from inside `on_detect`/the frame callback (`plugins/camera`'s own
//! doc). [`stop_all_streams`] is the one stop-before-start primitive every
//! mode switch and (re)start routes through, since [`stream_block`]'s own
//! raw stream and the scan strip's Policy/Timing streams all share the
//! session's single underlying stream claim (`plugins/camera/README.md` §4's
//! Occupancy table, [`crate::CameraError::StreamBusy`]) — this is what keeps
//! a mode toggle from ever hitting [`CameraError::StreamBusy`]. Torch
//! ([`torch_toggle_handler`], §5) is session-level and non-blocking; leaving
//! this page disposes [`CameraPage`]'s owner, which runs the same
//! `on_cleanup` that closes the session — [`CameraSession::close`] itself
//! stops any running stream and takes the torch out with it (both
//! documented on that method), so no extra teardown call is needed here.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use frust::{
    AnyView, AsyncValue, Axis, ButtonStyle, Color, Component, EdgeInsets, FlexChild, FlexView, Get,
    Padding, PlatformViewView, SizedBox, Theme, UseTask, any, button, component, inflexible,
    on_cleanup, platform_view, spawn_blocking, text, use_context, use_task,
};
use frust_camera::barcode::{Barcode, BarcodeFormat, decode_frame};
use frust_camera::{
    BarcodeStreamOptions, Camera, CameraError, CameraSession, DetectionPolicy, ImageFormat, Lens,
    PermissionStatus, Resolution,
};

// FrameTicker (mirrors `interactions.rs`'s `FrameTicker` / `appbar.rs`'s
// `AnchorReporter`) is a hand-rolled `View`/`Widget` pair — reached, like
// every other widget on this page, entirely through the `frust` facade.
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};

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
    /// Which [`ScanMode`] the scan strip currently targets — see the
    /// [module docs](self)'s Scan strip section.
    scan_mode: ScanMode,
    /// Whether the scan strip's own stream (of `scan_mode`'s kind) is
    /// believed running.
    scanning: bool,
    /// The last `start_barcode_stream`/`start_image_stream` error the scan
    /// strip hit, if any.
    scan_error: Option<String>,
    /// When the current scan-strip stream started, for Timing mode's
    /// attempts/sec readout.
    scan_started_at: Option<Instant>,
    /// [`ScanMode::Policy`]'s shared counters.
    policy_stats: Arc<PolicyStats>,
    /// [`ScanMode::Timing`]'s shared counters.
    timing_stats: Arc<TimingStats>,
    /// Torch requested-on state. Optimistic, not a device readback:
    /// `set_torch` is "accepted, not confirmed" (`plugins/camera/README.md`
    /// §5) — this mirrors the last call this page made, not a live LED
    /// state.
    torch_on: bool,
    /// The last `set_torch` error, if any.
    torch_error: Option<String>,
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
        // as `request_permission` above (the crate's own "Blocking API"
        // doc). Extracting the session is a brief-lock `Arc` *clone* (see
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
            scan_mode: ScanMode::Policy,
            scanning: false,
            scan_error: None,
            scan_started_at: None,
            policy_stats: Arc::new(PolicyStats::default()),
            timing_stats: Arc::new(TimingStats::default()),
            torch_on: false,
            torch_error: None,
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

        // Non-blocking, callable from any thread (`plugins/camera/README.md`
        // §5) — read fresh every rebuild rather than cached in state, since
        // Android's answer flips `true` only once CameraX finishes binding
        // (that section's own "re-check rather than latch its first answer"
        // note).
        let torch_available = state
            .session_cell
            .lock()
            .expect("session cell poisoned")
            .as_ref()
            .map(|session| session.torch_available())
            .unwrap_or(false);

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
            children.push(scan_block(state, torch_available));
            if state.streaming || state.scanning {
                // Keeps this Component's `build` re-invoked every frame while
                // a stream is (believed) running, so the frames/sec and scan
                // readouts stay live — see the [module docs](self)'s readout
                // sections.
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
/// off-screen and back — see `platform_views.rs`'s own `filler_rows` doc
/// comment for the device-viewport lesson behind the row count —
/// duplicated locally per that module's own precedent (private,
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
             the app is in the foreground."
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
// Lens switch
// ---------------------------------------------------------------------------

/// Close the current session (if any) and flip [`CameraPageState::lens`],
/// then clear [`CameraPageState::open_attempted`] so [`CameraPage::build`]'s
/// existing open branch — not a second one — reopens on the new lens next
/// rebuild. A no-op while no session is open (guarded by the early
/// `let...else` below), matching this control's "disabled (or a no-op)
/// while no session exists" contract.
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
    // nothing from it leaks into the new one.
    state.streaming = false;
    state.stream_started_at = None;
    state.stream_error = None;
    state.stream_stats.frames.store(0, Ordering::Relaxed);
    state.stream_stats.has_frame.store(false, Ordering::Relaxed);
    state.stream_stats.last_byte.store(0, Ordering::Relaxed);

    // The scan strip's stream (whichever kind) and the torch both belonged
    // to the outgoing session too — `session.close()` above already tore
    // the stream down and took the torch out with it; reset this page's own
    // belief state to match rather than showing a stale "scanning"/"torch
    // on" readout against the fresh session that hasn't started either yet.
    state.scanning = false;
    state.scan_error = None;
    state.scan_started_at = None;
    state
        .policy_stats
        .last
        .lock()
        .expect("policy stats poisoned")
        .take();
    state.policy_stats.count.store(0, Ordering::Relaxed);
    state
        .timing_stats
        .durations_us
        .lock()
        .expect("timing ring poisoned")
        .clear();
    state.timing_stats.attempts.store(0, Ordering::Relaxed);
    state.timing_stats.frame_w.store(0, Ordering::Relaxed);
    state.timing_stats.frame_h.store(0, Ordering::Relaxed);
    state.torch_on = false;
    state.torch_error = None;
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
/// `frust-paths` dependency added for this device-gate page.
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
// Scan strip — Policy mode (start_barcode_stream) / Timing mode
// (start_image_stream + barcode::decode_frame, hand-timed) — see the
// [module docs](self)'s Scan strip section.
// ---------------------------------------------------------------------------

/// The scan strip's two modes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScanMode {
    /// [`CameraSession::start_barcode_stream`] with
    /// [`DetectionPolicy::NoDuplicates`] — the muxr consumer shape end to
    /// end (`plugins/camera/README.md` §4).
    Policy,
    /// Raw [`CameraSession::start_image_stream`] +
    /// [`frust_camera::barcode::decode_frame`], hand-timed with
    /// [`std::time::Instant`] — the device measurement numbers.
    Timing,
}

/// One [`ScanMode::Policy`] detection's display fields — never the whole
/// [`Barcode`], so [`PolicyStats`] doesn't carry `corners`/`raw_bytes`
/// across the hand-off.
struct DetectedBarcode {
    raw_value: String,
    format: BarcodeFormat,
}

/// Shared counters [`start_policy_stream`]'s `on_detect` writes into — the
/// same plain-atomics/`Mutex` hand-off [`StreamStats`] already uses above
/// (never a signal write from inside the callback — `plugins/camera`'s own
/// doc).
#[derive(Default)]
struct PolicyStats {
    /// The most recent detection's display fields, or `None` before the
    /// first one arrives (or after a mode/lens switch resets it).
    last: Mutex<Option<DetectedBarcode>>,
    /// Total `on_detect` invocations since the current Policy-mode stream
    /// started.
    count: AtomicU64,
}

/// Ring-buffer capacity for [`TimingStats::durations_us`] — enough recent
/// samples for a stable median/p95 without growing unbounded.
const TIMING_RING_CAPACITY: usize = 128;

/// Shared counters [`start_timing_stream`]'s callback writes into, timing
/// [`decode_frame`] by hand with [`std::time::Instant`] — the same
/// atomics/`Mutex` hand-off [`PolicyStats`] uses.
#[derive(Default)]
struct TimingStats {
    /// The last [`TIMING_RING_CAPACITY`] decode durations, in whole
    /// microseconds, oldest-first.
    durations_us: Mutex<VecDeque<u64>>,
    /// Total decode attempts (every frame the callback runs) since the
    /// current Timing-mode stream started.
    attempts: AtomicU64,
    /// Most recent frame's width, in pixels.
    frame_w: AtomicU32,
    /// Most recent frame's height, in pixels.
    frame_h: AtomicU32,
}

/// Starts [`ScanMode::Policy`]: [`BarcodeFormat::QrCode`] only,
/// [`DetectionPolicy::NoDuplicates`] — proves the end-to-end muxr consumer
/// path. Resets `stats` first, matching [`start_stream`]'s own "clear
/// counters at (re)start" precedent.
fn start_policy_stream(
    session: &CameraSession,
    stats: &Arc<PolicyStats>,
) -> Result<(), CameraError> {
    stats.last.lock().expect("policy stats poisoned").take();
    stats.count.store(0, Ordering::Relaxed);
    let stats = Arc::clone(stats);
    session.start_barcode_stream(
        BarcodeStreamOptions {
            formats: vec![BarcodeFormat::QrCode],
            detection: DetectionPolicy::NoDuplicates,
        },
        move |found: &[Barcode]| {
            // `found` is never empty (the crate's own doc) — `first()` is
            // just the display convenience, not a defensive check.
            if let Some(barcode) = found.first() {
                *stats.last.lock().expect("policy stats poisoned") = Some(DetectedBarcode {
                    raw_value: barcode.raw_value.clone(),
                    format: barcode.format,
                });
            }
            stats.count.fetch_add(1, Ordering::Relaxed);
        },
    )
}

/// Starts [`ScanMode::Timing`]: a raw `Yuv420` stream with [`decode_frame`]
/// run and timed by hand inside the callback — produces the device
/// measurement numbers. Resets `stats` first, like [`start_policy_stream`].
fn start_timing_stream(
    session: &CameraSession,
    stats: &Arc<TimingStats>,
) -> Result<(), CameraError> {
    stats
        .durations_us
        .lock()
        .expect("timing ring poisoned")
        .clear();
    stats.attempts.store(0, Ordering::Relaxed);
    stats.frame_w.store(0, Ordering::Relaxed);
    stats.frame_h.store(0, Ordering::Relaxed);
    let stats = Arc::clone(stats);
    session.start_image_stream(ImageFormat::Yuv420, move |frame| {
        stats.frame_w.store(frame.width, Ordering::Relaxed);
        stats.frame_h.store(frame.height, Ordering::Relaxed);

        let started = Instant::now();
        let _ = decode_frame(frame, &[]);
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);

        stats.attempts.fetch_add(1, Ordering::Relaxed);
        let mut ring = stats.durations_us.lock().expect("timing ring poisoned");
        if ring.len() == TIMING_RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(elapsed_us);
    })
}

/// Releases every stream claim the session might hold — [`stream_block`]'s
/// own raw-stream claim above, and the scan strip's own barcode/raw claim,
/// whichever kind is actually active. `stop_image_stream`/
/// `stop_barcode_stream` are each a documented no-op unless *their* claim is
/// the one held (`plugins/camera/README.md` §4's Occupancy table), so
/// calling both unconditionally is safe — this is the "always stop the
/// current mode's stream before starting the other" primitive the
/// [module docs](self)'s Scan strip section calls for, and the one thing
/// every mode switch/(re)start below routes through so a mode toggle never
/// hits [`CameraError::StreamBusy`].
fn stop_all_streams(state: &mut CameraPageState) {
    {
        let guard = state.session_cell.lock().expect("session cell poisoned");
        if let Some(session) = guard.as_ref() {
            session.stop_image_stream();
            session.stop_barcode_stream();
        }
    }
    if state.streaming {
        state.streaming = false;
        state.stream_started_at = None;
    }
}

/// Starts `state.scan_mode`'s stream — call only right after
/// [`stop_all_streams`] released every claim, so this never observes
/// [`CameraError::StreamBusy`] from the scan strip's own prior stream.
fn start_scan(state: &mut CameraPageState) {
    let result = {
        let guard = state.session_cell.lock().expect("session cell poisoned");
        guard.as_ref().map(|session| match state.scan_mode {
            ScanMode::Policy => start_policy_stream(session, &state.policy_stats),
            ScanMode::Timing => start_timing_stream(session, &state.timing_stats),
        })
    };
    match result {
        Some(Ok(())) => {
            state.scanning = true;
            state.scan_started_at = Some(Instant::now());
            state.scan_error = None;
        }
        Some(Err(err)) => {
            state.scanning = false;
            state.scan_started_at = None;
            state.scan_error = Some(err.to_string());
        }
        None => {
            state.scanning = false;
            state.scan_started_at = None;
            state.scan_error = Some("no active camera session".to_string());
        }
    }
}

fn toggle_scan_handler(state: &mut CameraPageState) {
    if state.scanning {
        stop_all_streams(state);
        state.scanning = false;
        state.scan_started_at = None;
        return;
    }
    stop_all_streams(state);
    start_scan(state);
}

/// Switches [`ScanMode`] — always [`stop_all_streams`] first, so this never
/// hits [`CameraError::StreamBusy`] (module docs). A no-op stream-wise while
/// nothing is scanning: the mode just flips for the next
/// [`toggle_scan_handler`] press.
fn mode_toggle_handler(state: &mut CameraPageState) {
    let was_scanning = state.scanning;
    if was_scanning {
        stop_all_streams(state);
    }
    state.scan_mode = match state.scan_mode {
        ScanMode::Policy => ScanMode::Timing,
        ScanMode::Timing => ScanMode::Policy,
    };
    if was_scanning {
        start_scan(state);
    }
}

/// Torch is non-blocking and callable from any thread
/// (`plugins/camera/README.md` §5) — called directly from this UI-thread
/// handler, unlike the blocking `Camera::request_permission`/
/// `CameraSession::take_picture` pair. Optimistic on success (`set_torch` is
/// "accepted, not confirmed" — that section's own note), and leaves
/// `torch_on` unchanged on failure so the readout keeps reflecting the last
/// call that actually took.
fn torch_toggle_handler(state: &mut CameraPageState) {
    let guard = state.session_cell.lock().expect("session cell poisoned");
    let Some(session) = guard.as_ref() else {
        drop(guard);
        state.torch_error = Some("no active camera session".to_string());
        return;
    };
    let requested = !state.torch_on;
    let result = session.set_torch(requested);
    drop(guard);
    match result {
        Ok(()) => {
            state.torch_on = requested;
            state.torch_error = None;
        }
        Err(err) => state.torch_error = Some(err.to_string()),
    }
}

/// Percentile helper over a **sorted** snapshot of
/// [`TimingStats::durations_us`] — nearest-rank, `pct` in `0.0..=1.0`.
fn percentile_us(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (pct * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// Truncates `s` to at most `max_chars` characters for display, marking the
/// cut with an ellipsis — the dense muxr pairing fixture
/// (`plugins/camera/src/barcode/conformance.rs`'s `PAYLOAD_DENSE`, 176
/// chars) is exactly the case this exists for.
fn truncate_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}\u{2026}")
}

fn policy_readout(state: &CameraPageState) -> String {
    if let Some(err) = &state.scan_error {
        return format!("scan error: {err}");
    }
    if !state.scanning {
        return "scan stopped.".to_string();
    }
    let count = state.policy_stats.count.load(Ordering::Relaxed);
    let last = state
        .policy_stats
        .last
        .lock()
        .expect("policy stats poisoned");
    match last.as_ref() {
        Some(barcode) => format!(
            "{count} detection(s) \u{b7} last: {:?} \u{b7} \"{}\"",
            barcode.format,
            truncate_display(&barcode.raw_value, 40),
        ),
        None => format!("{count} detection(s) \u{b7} waiting for a QR code\u{2026}"),
    }
}

fn timing_readout(state: &CameraPageState) -> String {
    if let Some(err) = &state.scan_error {
        return format!("scan error: {err}");
    }
    if !state.scanning {
        return "scan stopped.".to_string();
    }

    let (last_us, sorted) = {
        let ring = state
            .timing_stats
            .durations_us
            .lock()
            .expect("timing ring poisoned");
        let Some(&last_us) = ring.back() else {
            return "streaming \u{2014} waiting for the first decode attempt\u{2026}".to_string();
        };
        let mut sorted: Vec<u64> = ring.iter().copied().collect();
        sorted.sort_unstable();
        (last_us, sorted)
    };

    let median_us = percentile_us(&sorted, 0.5);
    let p95_us = percentile_us(&sorted, 0.95);
    let attempts = state.timing_stats.attempts.load(Ordering::Relaxed);
    let width = state.timing_stats.frame_w.load(Ordering::Relaxed);
    let height = state.timing_stats.frame_h.load(Ordering::Relaxed);
    let attempts_per_sec = state
        .scan_started_at
        .map(|start| {
            let secs = start.elapsed().as_secs_f64();
            if secs > 0.0 {
                attempts as f64 / secs
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);

    format!(
        "decode: last {:.2}ms \u{b7} median {:.2}ms \u{b7} p95 {:.2}ms \u{b7} \
         {attempts_per_sec:.1} attempts/s \u{b7} frame {width}x{height}",
        last_us as f64 / 1000.0,
        median_us as f64 / 1000.0,
        p95_us as f64 / 1000.0,
    )
}

fn scan_block(state: &CameraPageState, torch_available: bool) -> FlexChild<CameraPageState> {
    let mode_label = match state.scan_mode {
        ScanMode::Policy => "Mode: Policy (tap for Timing)",
        ScanMode::Timing => "Mode: Timing (tap for Policy)",
    };
    let toggle_label = if state.scanning {
        "Stop scan"
    } else {
        "Start scan"
    };

    let mut rows: Vec<FlexChild<CameraPageState>> = vec![
        inflexible(label("Scan")),
        gap(6.0),
        inflexible(caption(
            "Policy mode proves the end-to-end muxr consumer path \
             (start_barcode_stream + NoDuplicates). Timing mode hand-times \
             barcode::decode_frame over a raw stream \u{2014} the Phase-4 \
             gate numbers.",
        )),
        gap(6.0),
        inflexible(any(button(mode_label, mode_toggle_handler)
            .style(ButtonStyle::Secondary)
            .small())),
        gap(6.0),
        inflexible(any(button(toggle_label, toggle_scan_handler)
            .style(ButtonStyle::Primary)
            .small())),
        gap(6.0),
    ];

    if torch_available {
        let torch_label = if state.torch_on {
            "Torch: on (tap to turn off)"
        } else {
            "Torch: off (tap to turn on)"
        };
        rows.push(inflexible(any(button(torch_label, torch_toggle_handler)
            .style(ButtonStyle::Secondary)
            .small())));
    } else {
        rows.push(inflexible(caption(
            "Torch unavailable \u{2014} no controllable flash on this lens/device.",
        )));
    }
    rows.push(gap(6.0));

    if let Some(err) = &state.torch_error {
        rows.push(inflexible(any(text(format!("torch error: {err}"))
            .size(11.0)
            .color(error_ink()))));
        rows.push(gap(6.0));
    }

    let readout = match state.scan_mode {
        ScanMode::Policy => policy_readout(state),
        ScanMode::Timing => timing_readout(state),
    };
    rows.push(inflexible(any(text(readout).size(11.0).color(muted()))));

    block(rows)
}

// ---------------------------------------------------------------------------
// FrameTicker — see `interactions.rs`'s twin for the full rationale.
// ---------------------------------------------------------------------------

/// A zero-size sentinel [`View`]/[`Widget`] pair whose only job is an
/// unconditional [`PaintCtx::request_frame`] call in its own `paint` —
/// mounted while [`CameraPageState::streaming`] or
/// [`CameraPageState::scanning`] is true, so the frames/sec readout in
/// [`stream_block`] and the detection/timing readouts in [`scan_block`]
/// keep refreshing (the [module docs](self)'s Image-stream readout and Scan
/// strip sections).
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
