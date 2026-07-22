//! Platform-agnostic shell plumbing shared by every Frust platform shell.
//!
//! The Android (`frust-shell-android`) and iOS shells both need the same
//! non-FFI machinery: the [`AppTree`] type-erasure that lets a non-generic
//! native handle drive any app's `State`/`app_logic`, a handful of pure
//! helpers for crossing an FFI boundary safely ([`guard`]) and turning an
//! untrusted density into HiDPI layout math ([`sanitize_scale`]/[`logical_size`]/
//! [`logical_insets`], the last converting platform per-edge insets into a
//! logical [`WindowInsets`](frust_core::insets::WindowInsets)),
//! the app-facing theme override slot ([`set_app_theme`]/[`clear_app_theme`]/
//! [`ThemeOverrideWatcher`] — see [`theme_override`]'s module docs for the
//! layering rationale), the [`perf`] module's frame-timing/startup-span
//! instrumentation, the [`frame_gate`] module's shared skip-frame decision
//! ([`FrameGate`]/[`FrameInputs`]/[`FrameDecision`]) the mobile shells consult
//! to idle on unchanged frames (spec §14 phase 7), the [`resample`]
//! module's pointer-event resampling ([`PointerResampler`]) plus deadline-aware
//! pacing helpers the mobile shells drive from their touch + frame paths (plan
//! phase 10.C), and the [`render_split`] module's UI→render-thread plumbing
//! ([`render_channel`]'s latest-wins scene handoff + [`RenderCommand`]/[`Ack`]
//! lifecycle vocabulary, gated by [`render_thread_enabled`], plus
//! [`scene_return_channel`]'s reverse give-back slot restoring buffer reuse
//! across the split) the shells split the frame pipeline across (plan phase
//! 11.B).
//!
//! This crate is deliberately platform-free: it depends on `frust-core`
//! (retained tree / `RenderRoot`) plus `frust-scene`/`frust-text`/
//! `frust-theme` for the types a shell composes around it, but never on
//! `jni`/`ndk`/`winit`. It contains no `unsafe` and no FFI, so it compiles
//! unchanged on the host and on `aarch64-linux-android`/iOS with no cfg
//! gymnastics. Each platform shell keeps its own FFI boundary (JNI exports,
//! raw-window handling) and reuses this crate rather than duplicating the
//! plumbing.

mod app_tree;
mod ffi_support;
pub mod font_registry;
pub mod frame_gate;
pub mod perf;
pub mod render_split;
pub mod resample;
mod theme_override;

pub use app_tree::{AppTree, new_boxed_app, new_boxed_app_with};
pub use ffi_support::{guard, logical_insets, logical_size, run_guarded_thread, sanitize_scale};
pub use frame_gate::{FrameDecision, FrameGate, FrameInputs};
pub use render_split::{
    Ack, AckWaiter, FrameMeta, NO_RENDER_THREAD_VAR, RenderBatch, RenderCommand, RenderEvent,
    RenderPhase, RenderReceiver, RenderSender, SceneFrame, SceneReturnReceiver, SceneReturnSender,
    SurfaceSize, ack_pair, next_render_phase, render_channel, render_thread_enabled,
    scene_return_channel,
};
pub use resample::{PointerResampler, RawPointerSample};
pub use theme_override::{
    ThemeOverrideWatcher, clear_app_theme, effective_brightness_for_platform_change, set_app_theme,
    theme_override_active,
};
