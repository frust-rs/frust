//! Video page: `frust-video-player`'s device-gate vehicle — a source picker,
//! a real `video_view` slot, and transport/scrubber/state chrome below it, on
//! Android, iOS **and** the macOS desktop shell.
//!
//! # Sources
//!
//! Three buttons open a fresh session immediately
//! ([`frust_video_player::VideoPlayer::open`] is non-blocking, so the button
//! handler below never spawns anything):
//!
//! - **https MP4** ([`HTTPS_SAMPLE_URL`]) — MDN's own `flower.mp4`
//!   interactive-example clip, served over https from
//!   `interactive-examples.mdn.mozilla.net` and released under CC0 (MDN's
//!   `cc0-videos` set — verified reachable, `video/mp4`, ~1.1 MB, so this is
//!   the streamed-download source, not the bundled asset below).
//! - **HLS** ([`HLS_SAMPLE_URL`]) — Apple's own published fMP4
//!   bipbop example stream, verified live and stable.
//! - **Asset** — [`VideoSource::Asset`] naming `frust-video-sample.mp4`,
//!   bundled in both native projects (`android/app/src/main/assets/` and
//!   `ios/Runner/`). This clip is generated locally with `ffmpeg`'s
//!   `testsrc`/`sine` `lavfi` sources rather than a downloaded CC0 file, so
//!   the offline `Asset` path never depends on network reachability:
//!   `ffmpeg -f lavfi -i testsrc=size=320x180:rate=30:duration=3 -f lavfi -i
//!   sine=frequency=440:duration=3 -c:v libx264 -pix_fmt yuv420p -profile:v
//!   baseline -level 3.0 -crf 32 -preset veryslow -movflags +faststart -c:a
//!   aac -b:a 32k -shortest frust-video-sample.mp4` — 3 seconds, 320×180,
//!   H.264/AAC, 29,450 bytes (well under the 300 KB ceiling). On macOS an
//!   unbundled `cargo run` binary has no `NSBundle.mainBundle()` to resolve
//!   an asset name against, so [`asset_source`] falls back to a `File` path
//!   next to the running executable there instead; a `frust build macos` app
//!   bundle carries the clip as a real bundle resource, where
//!   `VideoSource::Asset` resolves normally.
//!
//! # Layout
//!
//! The slot sits in a fixed 320×180 (16:9) box
//! ([`VIDEO_W`]/[`VIDEO_H`]) — `frust` ships no `AspectRatio` widget
//! yet, so a fixed box is the same honest choice `pages::camera`'s own
//! `PREVIEW_W`/`PREVIEW_H` already made for the same reason. Every control
//! (play/pause, ±10s, rate, volume, loop, fit, close), the scrubber, the
//! state chip and the video-size readout live in a column BELOW the slot,
//! never on top of it — [`frust_video_player::api::video_view`]'s own
//! doc explains why: macOS hosts the picture as an opaque native sibling
//! (Mode A), so anything a `Stack` placed over the slot there would be
//! physically hidden underneath the player. On Android/iOS only, one small
//! state chip IS still overlaid in a `Stack`, placed AFTER the slot in that
//! stack's child list — see [`mobile_state_chip`]'s own doc comment for
//! why that ordering (not a Z-shield: `video_view` never calls
//! `.interactive()`, so the slot stays in `frust`'s own hit-test path) is
//! what keeps the chip actually visible over the punched-through native
//! picture there.
//!
//! # The scrubber seeks on every reported value, not strictly on release
//!
//! [`frust::slider`]'s own contract has no distinct "drag ended" event to
//! defer a seek to: the widget fires `on_change` on every `Down` and
//! captured `Move`, and reports nothing at all on `Up`. [`seek_handler`]
//! below therefore issues [`frust_video_player::api::VideoPlayerHandle::seek_to`]
//! on every reported fraction — a strict superset of "seek on release" (the
//! gesture's final `on_change` call already lands at the released position;
//! this only adds intermediate seeks during the drag itself, not a missing
//! one at its end).
//!
//! # Desktop-only note
//!
//! On macOS (the one desktop target this crate's `apple` backend actually
//! serves — Linux/Windows have no video backend at all, crate doc's
//! *Backends* section), [`desktop_wheel_note`] adds one caption explaining
//! that scrolling the mouse wheel over the video does not scroll this page:
//! the picture is hosted as an opaque native sibling (Mode A) sitting fully
//! above the whole `frust` surface, so a wheel event over its bounds never
//! reaches `frust`'s own scroll view underneath.

#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::time::Duration;

#[cfg(any(target_os = "android", target_os = "ios"))]
use frust::{Align, Alignment, Stack, container};
use frust::{
    AnyView, Axis, ButtonStyle, Color, Component, EdgeInsets, FlexChild, FlexView, Get,
    GetUntracked, Padding, PlatformViewView, SizedBox, Theme, View, any, button, component,
    inflexible, slider, text, use_context,
};
use frust_video_player::api::{VideoPlayerHandle, video_view};
use frust_video_player::{
    PlaybackState, PlayerOptions, VideoError, VideoFit, VideoPlayer, VideoSource,
};

use crate::PlaygroundState;

/// A short, publicly reachable https MP4 — MDN's own CC0 `flower.mp4`
/// interactive-example clip. See the [module docs](self)'s Sources section.
const HTTPS_SAMPLE_URL: &str =
    "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.mp4";

/// Apple's own published fMP4 bipbop advanced example stream —
/// verified live, chosen because it needs no hosting of this crate's own.
const HLS_SAMPLE_URL: &str = "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_fmp4/master.m3u8";

/// The bundled clip's asset name — matches the file this crate ships at
/// `android/app/src/main/assets/frust-video-sample.mp4` and
/// `ios/Runner/frust-video-sample.mp4`.
const ASSET_NAME: &str = "frust-video-sample.mp4";

/// The slot's fixed 16:9 size, in logical px — see the [module
/// docs](self)'s Layout section for why this is a fixed box rather than an
/// `AspectRatio` wrapper.
const VIDEO_W: f64 = 320.0;
const VIDEO_H: f64 = 180.0;

/// Amount `-10s`/`+10s` step the position by.
const SEEK_STEP: Duration = Duration::from_secs(10);

/// `set_volume`/`set_rate` step size for the volume buttons.
const VOLUME_STEP: f32 = 0.25;

/// See the page-fn contract in [`crate::pages`]. Video has no shared signal
/// to read — it only mounts [`VideoPlayerPage`]'s own retained local
/// state.
pub fn page(_state: &PlaygroundState) -> AnyView<PlaygroundState> {
    any(component(VideoPlayerPage))
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

fn https_source() -> VideoSource {
    VideoSource::Url(HTTPS_SAMPLE_URL.to_owned())
}

fn hls_source() -> VideoSource {
    VideoSource::Url(HLS_SAMPLE_URL.to_owned())
}

/// See the [module docs](self)'s Sources section for the macOS
/// `cargo run`-vs-bundle distinction this implements.
#[cfg(target_os = "macos")]
fn asset_source() -> VideoSource {
    match std::env::current_exe() {
        Ok(exe) => {
            let path = exe
                .parent()
                .map(|dir| dir.join(ASSET_NAME))
                .unwrap_or_else(|| PathBuf::from(ASSET_NAME));
            VideoSource::File(path)
        }
        // No executable path available — fall back to the ordinary
        // asset lookup rather than failing outright.
        Err(_) => VideoSource::Asset(ASSET_NAME.to_owned()),
    }
}

#[cfg(not(target_os = "macos"))]
fn asset_source() -> VideoSource {
    VideoSource::Asset(ASSET_NAME.to_owned())
}

// ---------------------------------------------------------------------------
// Retained state + Component
// ---------------------------------------------------------------------------

/// [`VideoPlayerPage`]'s retained local state.
struct VideoPageState {
    /// The open session's reactive handle, or `None` before any source has
    /// been opened. Dropping this (replacing it with a fresh handle, or the
    /// component's own teardown dropping `State`) unregisters its listener
    /// before its session's own `Drop` closes the platform player —
    /// `VideoPlayerHandle`'s own doc: field order is load-bearing, so a plain
    /// `Option` field here needs no extra `on_cleanup` registration of its
    /// own to release the player at teardown.
    handle: Option<VideoPlayerHandle>,
    /// [`VideoPlayer::open`]'s error, if the last open attempt failed.
    open_error: Option<String>,
    /// The last control-method (`play`/`pause`/`seek_to`/`set_rate`/
    /// `set_volume`/`set_looping`) error, if any — distinct from
    /// [`VideoPlayerHandle::error`], which only ever reports a platform
    /// playback failure.
    control_error: Option<String>,
    /// How the picture fills the slot — purely a `video_view` build
    /// argument (see that function's own doc: no separate control call
    /// needed), so this stays a plain field, not a signal.
    fit: VideoFit,
    /// The last rate this page requested — optimistic, like
    /// `pages::camera`'s own `torch_on`: `PlayerSnapshot` carries no rate
    /// field to read back.
    rate: f32,
    /// The last volume this page requested — optimistic, same reason.
    volume: f32,
    /// The last looping flag this page requested — optimistic, same
    /// reason.
    looping: bool,
}

/// The video page's own [`Component`] (see the [module docs](self)).
struct VideoPlayerPage;

impl Component for VideoPlayerPage {
    type State = VideoPageState;

    fn init(&self) -> VideoPageState {
        VideoPageState {
            handle: None,
            open_error: None,
            control_error: None,
            fit: VideoFit::Contain,
            rate: 1.0,
            volume: 1.0,
            looping: false,
        }
    }

    fn build(&self, state: &mut VideoPageState) -> impl View<VideoPageState> {
        let mut children: Vec<FlexChild<VideoPageState>> = vec![
            block(vec![
                inflexible(label("Video")),
                gap(6.0),
                inflexible(caption(
                    "frust-video-player's device-gate page \u{2014} pick a source below to open \
                     a session, then scroll this slot off/on-screen to exercise its real \
                     dispose/revive cycle.",
                )),
            ]),
            source_picker_block(state),
        ];

        if let Some(handle) = state.handle.as_ref() {
            let playback_state = handle.state.get();
            let position = handle.position.get();
            let duration = handle.duration.get();
            let video_size = handle.video_size.get();
            let error = handle.error.get();

            children.push(slot_block(handle, state.fit, playback_state));
            children.push(transport_block(state, playback_state));
            children.push(scrubber_block(position, duration));
            children.push(state_chip_block(
                playback_state,
                error.as_ref(),
                state.control_error.as_deref(),
            ));
            children.push(video_size_block(video_size));
        }

        #[cfg(target_os = "macos")]
        children.push(desktop_wheel_note());

        children.push(gap(12.0));
        children.push(inflexible(filler_rows()));

        any(Padding(
            EdgeInsets::all(16.0),
            FlexView::new(Axis::Vertical, children),
        ))
    }
}

// ---------------------------------------------------------------------------
// Section chrome (mirrors `platform_views.rs`/`camera.rs`'s identical
// helpers — duplicated per-module by established convention, not
// shared)
// ---------------------------------------------------------------------------

fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

fn error_ink() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .error
}

fn label(s: impl Into<String>) -> AnyView<VideoPageState> {
    any(text(s).size(13.0).color(accent()))
}

fn caption(s: impl Into<String>) -> AnyView<VideoPageState> {
    any(text(s).size(11.0).color(muted()))
}

fn gap(h: f64) -> FlexChild<VideoPageState> {
    inflexible(SizedBox(None, Some(h)))
}

fn gap_h(w: f64) -> FlexChild<VideoPageState> {
    inflexible(SizedBox(Some(w), None))
}

fn block(children: Vec<FlexChild<VideoPageState>>) -> FlexChild<VideoPageState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

/// A bounded list of filler rows so the slot can scroll fully off-screen and
/// back — see `platform_views.rs`'s own `filler_rows` doc comment for
/// the device-viewport lesson behind the row count — duplicated
/// locally per that module's own precedent.
fn filler_rows() -> AnyView<VideoPageState> {
    let rows: Vec<AnyView<VideoPageState>> = (1..=64)
        .map(|i| {
            any(Padding(
                EdgeInsets::symmetric(0.0, 8.0),
                text(format!("row {i:02} \u{2014} video page filler")).size(12.0),
            ))
        })
        .collect();
    any(FlexView::new(
        Axis::Vertical,
        rows.into_iter().map(inflexible).collect(),
    ))
}

/// Format a [`Duration`] as `mm:ss`, truncating sub-second precision.
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

// ---------------------------------------------------------------------------
// Source picker
// ---------------------------------------------------------------------------

fn open_with(state: &mut VideoPageState, source: VideoSource) {
    // Replace any existing handle first — dropping it unregisters its
    // listener before its session's own `Drop` closes the platform player
    // (`VideoPlayerHandle`'s own doc: field order is load-bearing), so this
    // never leaves two sessions open at once.
    state.handle = None;
    state.open_error = None;
    state.control_error = None;
    state.fit = VideoFit::Contain;
    state.rate = 1.0;
    state.volume = 1.0;
    state.looping = false;

    match VideoPlayer::open(source, PlayerOptions::default()) {
        Ok(session) => state.handle = Some(VideoPlayerHandle::new(session)),
        Err(err) => state.open_error = Some(err.to_string()),
    }
}

fn open_https_handler(state: &mut VideoPageState) {
    open_with(state, https_source());
}

fn open_hls_handler(state: &mut VideoPageState) {
    open_with(state, hls_source());
}

fn open_asset_handler(state: &mut VideoPageState) {
    open_with(state, asset_source());
}

fn source_picker_block(state: &VideoPageState) -> FlexChild<VideoPageState> {
    let mut rows = vec![
        inflexible(label("Source")),
        gap(6.0),
        inflexible(caption(
            "VideoPlayer::open is non-blocking \u{2014} each button opens a session \
             immediately and returns before it has loaded.",
        )),
        gap(6.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button("https MP4", open_https_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("HLS", open_hls_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("Asset", open_asset_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
            ],
        ))),
    ];

    if let Some(err) = &state.open_error {
        rows.push(gap(6.0));
        rows.push(inflexible(any(text(format!("open error: {err}"))
            .size(11.0)
            .color(error_ink()))));
    } else if state.handle.is_none() {
        rows.push(gap(6.0));
        rows.push(inflexible(caption("No session open yet.")));
    }

    block(rows)
}

// ---------------------------------------------------------------------------
// Slot
// ---------------------------------------------------------------------------

/// Apply [`PlatformViewView::debug_fill`] only in a debug build — see
/// `platform_views.rs`'s identical `maybe_debug_fill` precedent: the method
/// doesn't exist under a release profile.
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

/// The Android/iOS-only overlaid state chip — see the [module
/// docs](self)'s Layout section.
///
/// Placed AFTER the slot in [`slot_block`]'s `Stack` on purpose: under
/// Playground's Mode B translucent surface (`crate`'s own module docs' "Mode
/// B background" section) the slot punches a real hole straight through to
/// the native player layer beneath, and a `Stack` paints its children in
/// list order — a chip painted BEFORE the slot would have its own fill
/// painted first, then be punched straight through by the slot's hole
/// (invisible), rather than sitting on top of the picture as chrome. Square
/// corners (`.radius(0.0)`, `ContainerView`'s own default) rather than the
/// design system's usual rounded chip: an engine finding
/// recorded a rounded fill compositing
/// incorrectly at a punched-hole edge, so this overlay avoids that shape
/// here.
#[cfg(any(target_os = "android", target_os = "ios"))]
fn mobile_state_chip(playback_state: PlaybackState) -> AnyView<VideoPageState> {
    let chip = container(Padding(
        EdgeInsets::symmetric(3.0, 6.0),
        text(format!("{playback_state:?}"))
            .size(10.0)
            .color(Color::from_rgb8(0xFF, 0xFF, 0xFF)),
    ))
    .fill(Color::new([0.0, 0.0, 0.0, 0.6]))
    .radius(0.0);

    any(Align(Alignment::TOP_RIGHT, chip))
}

fn slot_block(
    handle: &VideoPlayerHandle,
    fit: VideoFit,
    _playback_state: PlaybackState,
) -> FlexChild<VideoPageState> {
    let slot = maybe_debug_fill(video_view(handle, fit).size(VIDEO_W, VIDEO_H));

    #[cfg(any(target_os = "android", target_os = "ios"))]
    let slot_view: AnyView<VideoPageState> =
        any(Stack(vec![any(slot), mobile_state_chip(_playback_state)]));
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let slot_view: AnyView<VideoPageState> = any(slot);

    block(vec![
        inflexible(label("Picture")),
        gap(6.0),
        inflexible(slot_view),
    ])
}

// ---------------------------------------------------------------------------
// Transport controls
// ---------------------------------------------------------------------------

fn play_pause_handler(state: &mut VideoPageState) {
    let Some(handle) = state.handle.as_ref() else {
        return;
    };
    let result = if handle.state.get_untracked() == PlaybackState::Playing {
        handle.pause()
    } else {
        handle.play()
    };
    state.control_error = result.err().map(|err| err.to_string());
}

fn seek_back_handler(state: &mut VideoPageState) {
    let Some(handle) = state.handle.as_ref() else {
        return;
    };
    let target = handle.position.get_untracked().saturating_sub(SEEK_STEP);
    state.control_error = handle.seek_to(target).err().map(|err| err.to_string());
}

fn seek_forward_handler(state: &mut VideoPageState) {
    let Some(handle) = state.handle.as_ref() else {
        return;
    };
    let target = handle.position.get_untracked() + SEEK_STEP;
    let clamped = match handle.duration.get_untracked() {
        Some(duration) => target.min(duration),
        None => target,
    };
    state.control_error = handle.seek_to(clamped).err().map(|err| err.to_string());
}

/// Builds a `set_rate(rate)` handler for one of the four fixed rate buttons.
fn rate_handler(rate: f32) -> impl Fn(&mut VideoPageState) {
    move |state: &mut VideoPageState| {
        let Some(handle) = state.handle.as_ref() else {
            return;
        };
        match handle.set_rate(rate) {
            Ok(()) => {
                state.rate = rate;
                state.control_error = None;
            }
            Err(err) => state.control_error = Some(err.to_string()),
        }
    }
}

fn volume_step_handler(delta: f32) -> impl Fn(&mut VideoPageState) {
    move |state: &mut VideoPageState| {
        let Some(handle) = state.handle.as_ref() else {
            return;
        };
        let target = (state.volume + delta).clamp(0.0, 1.0);
        match handle.set_volume(target) {
            Ok(()) => {
                state.volume = target;
                state.control_error = None;
            }
            Err(err) => state.control_error = Some(err.to_string()),
        }
    }
}

fn loop_toggle_handler(state: &mut VideoPageState) {
    let Some(handle) = state.handle.as_ref() else {
        return;
    };
    let target = !state.looping;
    match handle.set_looping(target) {
        Ok(()) => {
            state.looping = target;
            state.control_error = None;
        }
        Err(err) => state.control_error = Some(err.to_string()),
    }
}

/// Toggles [`VideoPageState::fit`] only — no session control call
/// (see the [module docs](self)'s Layout section: `video_view` reads `fit`
/// straight from `build`'s own argument).
fn fit_toggle_handler(state: &mut VideoPageState) {
    state.fit = match state.fit {
        VideoFit::Contain => VideoFit::Cover,
        VideoFit::Cover => VideoFit::Contain,
    };
}

/// Releases the platform player but keeps the handle itself alive, so its
/// still-registered listener receives the backend's own final `Idle`
/// publish and the state chip below reflects it — dropping the handle
/// instead (unregistering the listener first) would make that transition
/// unobservable, per `VideoPlayerHandle`'s own doc.
fn close_handler(state: &mut VideoPageState) {
    if let Some(handle) = state.handle.as_ref() {
        handle.close();
    }
    state.control_error = None;
}

fn transport_block(
    state: &VideoPageState,
    playback_state: PlaybackState,
) -> FlexChild<VideoPageState> {
    let play_pause_label = if playback_state == PlaybackState::Playing {
        "Pause"
    } else {
        "Play"
    };
    let loop_label = if state.looping {
        "Loop: on"
    } else {
        "Loop: off"
    };
    let fit_label = match state.fit {
        VideoFit::Contain => "Fit: contain",
        VideoFit::Cover => "Fit: cover",
    };

    block(vec![
        inflexible(label("Transport")),
        gap(6.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button(play_pause_label, play_pause_handler)
                    .style(ButtonStyle::Primary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("\u{2212}10s", seek_back_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("+10s", seek_forward_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("Close", close_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
            ],
        ))),
        gap(6.0),
        inflexible(caption(format!("Rate: {:.2}x", state.rate))),
        gap(4.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button("0.5x", rate_handler(0.5))
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("1x", rate_handler(1.0))
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("1.5x", rate_handler(1.5))
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button("2x", rate_handler(2.0))
                    .style(ButtonStyle::Secondary)
                    .small())),
            ],
        ))),
        gap(6.0),
        inflexible(caption(format!("Volume: {:.0}%", state.volume * 100.0))),
        gap(4.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button(
                    "Vol \u{2212}",
                    volume_step_handler(-VOLUME_STEP),
                )
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button("Vol +", volume_step_handler(VOLUME_STEP))
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button(loop_label, loop_toggle_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button(fit_label, fit_toggle_handler)
                    .style(ButtonStyle::Secondary)
                    .small())),
            ],
        ))),
    ])
}

// ---------------------------------------------------------------------------
// Scrubber
// ---------------------------------------------------------------------------

/// See the [module docs](self)'s "The scrubber seeks on every reported
/// value" section for why this fires on every drag update rather than
/// deferring to a release event the underlying `Slider` widget never
/// reports.
fn seek_handler(state: &mut VideoPageState, fraction: f64) {
    let Some(handle) = state.handle.as_ref() else {
        return;
    };
    let Some(duration) = handle.duration.get_untracked() else {
        return;
    };
    let target_secs = duration.as_secs_f64() * fraction.clamp(0.0, 1.0);
    state.control_error = handle
        .seek_to(Duration::from_secs_f64(target_secs.max(0.0)))
        .err()
        .map(|err| err.to_string());
}

fn scrubber_block(position: Duration, duration: Option<Duration>) -> FlexChild<VideoPageState> {
    let duration_secs = duration.map(|d| d.as_secs_f64()).filter(|s| *s > 0.0);
    let fraction = duration_secs
        .map(|total| (position.as_secs_f64() / total).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let readout = format!(
        "{} / {}",
        format_duration(position),
        duration
            .map(format_duration)
            .unwrap_or_else(|| "--:--".to_owned()),
    );

    block(vec![
        inflexible(label("Scrubber")),
        gap(6.0),
        inflexible(any(slider(fraction, seek_handler))),
        gap(6.0),
        inflexible(caption(readout)),
    ])
}

// ---------------------------------------------------------------------------
// State chip + video-size readout
// ---------------------------------------------------------------------------

fn state_chip_block(
    playback_state: PlaybackState,
    playback_error: Option<&VideoError>,
    control_error: Option<&str>,
) -> FlexChild<VideoPageState> {
    let state_text = format!("State: {playback_state:?}");
    let color = if playback_state == PlaybackState::Error {
        error_ink()
    } else {
        muted()
    };

    let mut rows = vec![
        inflexible(label("Status")),
        gap(6.0),
        inflexible(any(text(state_text).size(12.0).color(color))),
    ];

    if let Some(err) = playback_error {
        rows.push(gap(4.0));
        rows.push(inflexible(any(text(format!("playback error: {err}"))
            .size(11.0)
            .color(error_ink()))));
    }
    if let Some(err) = control_error {
        rows.push(gap(4.0));
        rows.push(inflexible(any(text(format!("control error: {err}"))
            .size(11.0)
            .color(error_ink()))));
    }

    block(rows)
}

fn video_size_block(video_size: Option<(u32, u32)>) -> FlexChild<VideoPageState> {
    let readout = match video_size {
        Some((w, h)) => format!("{w}\u{d7}{h}"),
        None => "unknown \u{2014} no frame decoded yet".to_owned(),
    };
    block(vec![
        inflexible(label("Video size")),
        gap(6.0),
        inflexible(caption(readout)),
    ])
}

// ---------------------------------------------------------------------------
// Desktop-only note
// ---------------------------------------------------------------------------

/// See the [module docs](self)'s Desktop-only note section.
#[cfg(target_os = "macos")]
fn desktop_wheel_note() -> FlexChild<VideoPageState> {
    block(vec![inflexible(caption(
        "On desktop the picture is an opaque native sibling above the whole frust surface \
         (platform-view Mode A) \u{2014} scrolling the mouse wheel over it does not scroll this \
         page.",
    ))])
}
