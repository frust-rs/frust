//! Browser shell: the fourth host tier, alongside desktop, Android and iOS.
//!
//! The browser is a winit host like the desktop is, but it is not a *desktop*
//! host: there is no blocking executor to bring a surface up with, no render
//! thread, no on-disk pipeline cache, and no `accesskit_winit` adapter. So this
//! crate does **not** depend on `frust-shell-desktop`. It sits directly on
//! `frust-shell-common`, the same way `frust-shell-android` and
//! `frust-shell-ios` do, and carries its own copy of the winit-generic halves
//! of that desktop core — input mapping, the reactive-owner event wrap, the
//! change-guarded window-metrics publish, and brightness-follow. See
//! `docs/SHELLS_ARCHITECTURE.md` for how SHELLS is layered.
//!
//! # What this crate is today
//!
//! Two layers. The host-signal translation layer ([`app_handler`]'s
//! winit-generic half, [`pacing`], [`ime`]'s policy and mapping, and
//! [`render`]'s pure pieces) is built out of pure or near-pure functions that
//! run and are unit-tested on the build host as well as on
//! `wasm32-unknown-unknown`, which is what keeps the browser translation
//! testable without a browser. On top of it, gated to `target_arch = "wasm32"`,
//! sits the part that owns browser resources: the canvas-bound event loop and
//! `requestAnimationFrame` frame pipeline ([`spawn_app`], entered by the facade
//! through [`run_app`]), the single-thread inline frame executor over the wgpu
//! surface ([`render::WebFrameExecutor`]), and the input method's own DOM
//! element (`ime::ImeOverlay`).
//!
//! # Host signals a browser does not have
//!
//! Two of the desktop core's seams have no web counterpart at all, and are
//! honest documented no-ops here rather than absent or faked — see
//! [`app_handler::push_semantics`] and [`app_handler::pump_devtools`] for each
//! one's own reasoning. Two others *do* have one: cursor setting, which winit's
//! web backend drives through the canvas's CSS `cursor` property
//! ([`app_handler::sync_cursor`]), and the input method, which winit does not
//! reach at all — [`ime`] bypasses it with a hidden `<input>` overlay bound to
//! the browser's own composition events.
//!
//! # Default text face
//!
//! [`fonts::install_default_fonts`] bundles a default face and registers it
//! as this target's `SystemUi`/`SansSerif` generic-family fallback ahead of
//! the shell's `TextContext` construction — see that module's docs for why
//! this target needs it (fontique's system-font backend has no generic-family
//! map on `wasm32`).

pub mod app_handler;
pub mod fonts;
pub mod ime;
pub mod input;
pub mod logging;
pub mod pacing;
pub mod render;

pub use app_handler::{
    ComposeLatch, InputState, apply_theme, base_theme, brightness_from_winit, event_under_owner,
    follow_platform_brightness, publish_window_metrics, pump_devtools, push_semantics,
    reverted_theme, sync_cursor, theme_after_override_poll,
};
#[cfg(target_arch = "wasm32")]
pub use app_handler::{ShellUserEvent, run_app, spawn_app};
pub use fonts::install_default_fonts;
#[cfg(target_arch = "wasm32")]
pub use ime::ImeOverlay;
pub use ime::{
    DomEditEvent, IME_PROCESS_KEY, MIN_OVERLAY_SIDE, OverlayAction, OverlayBox, OverlayPolicy,
    UNIDENTIFIED_KEY, cancels_composition, forwards_to_canvas, overlay_box, overlay_box_moved,
    session_is_active,
};
pub use input::{TouchTracker, map_touch_phase};
pub use logging::{DEFAULT_LEVEL, LEVEL_QUERY_PARAM, install, level_from_query, parse_level};
pub use pacing::{
    ControlFlowIntent, OVERSHOOT_LOG_THRESHOLD, PacedDecision, RAF_PERIOD_60HZ, next_paced_wake,
    overshoot, paced_interval, paced_wake_action, raf_quantized,
};
pub use render::{
    BRINGUP_ATTEMPTS, BRINGUP_RETRY_INTERVAL, FrameFollowUp, WebFrameExecutor, bringup_retry_delay,
    follow_up_for,
};
