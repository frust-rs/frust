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
//! The host-signal translation layer only. The pieces that own a browser
//! resource — the canvas-bound event loop, the `requestAnimationFrame` frame
//! pipeline, and the wgpu surface — are not here yet; [`app_handler`]'s
//! contents are what those are built out of, and every one of them is a pure or
//! near-pure function that runs and is unit-tested on the build host as well as
//! on `wasm32-unknown-unknown`. Nothing here is `cfg`-gated to wasm: the whole
//! surface compiles for both, which is what keeps the browser translation
//! testable without a browser.
//!
//! # Host signals a browser does not have
//!
//! Three of the desktop core's seams have no web counterpart at all, and are
//! honest documented no-ops here rather than absent or faked — see
//! [`app_handler::sync_ime`], [`app_handler::push_semantics`] and
//! [`app_handler::pump_devtools`] for each one's own reasoning. A fourth,
//! cursor setting, *does* have one (winit's web backend drives the canvas's CSS
//! `cursor` property), so it is a real implementation:
//! [`app_handler::sync_cursor`].

pub mod app_handler;
pub mod pacing;
pub mod render;

pub use app_handler::{
    ComposeLatch, InputState, apply_theme, base_theme, brightness_from_winit, event_under_owner,
    follow_platform_brightness, publish_window_metrics, pump_devtools, push_semantics,
    reverted_theme, sync_cursor, sync_ime, theme_after_override_poll,
};
#[cfg(target_arch = "wasm32")]
pub use app_handler::{ShellUserEvent, spawn_app};
pub use pacing::{
    ControlFlowIntent, OVERSHOOT_LOG_THRESHOLD, PacedDecision, RAF_PERIOD_60HZ, next_paced_wake,
    overshoot, paced_interval, paced_wake_action, raf_quantized,
};
pub use render::{
    BRINGUP_ATTEMPTS, BRINGUP_RETRY_INTERVAL, FrameFollowUp, WebFrameExecutor, bringup_retry_delay,
    follow_up_for,
};
