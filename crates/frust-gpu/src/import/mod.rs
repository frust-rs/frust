//! Zero-copy texture import — platform buffers (a CVPixelBuffer on Apple, an
//! AHardwareBuffer on Android) wrapped as `wgpu::Texture`s without a copy, for the
//! external-texture frame bus (render-model Phase 4, decision D11). Each platform
//! half lives behind its own `cfg`; this module is empty on every other target.
//! Each half is filled in by its platform's zero-copy import work.

#[cfg(target_vendor = "apple")]
pub mod apple;

#[cfg(target_os = "android")]
pub mod android;
