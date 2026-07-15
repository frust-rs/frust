//! Platform-agnostic shell plumbing shared by every ForgeKit platform shell.
//!
//! The Android (`forgekit-shell-android`) and iOS shells both need the same
//! non-FFI machinery: the [`AppTree`] type-erasure that lets a non-generic
//! native handle drive any app's `State`/`app_logic`, and a handful of pure
//! helpers for crossing an FFI boundary safely ([`guard`]) and turning an
//! untrusted density into HiDPI layout math ([`sanitize_scale`]/[`logical_size`]).
//!
//! This crate is deliberately platform-free: it depends on `forgekit-core`
//! (retained tree / `RenderRoot`) plus `forgekit-scene`/`forgekit-text` for the
//! types a shell composes around it, but never on `jni`/`ndk`/`winit`. It
//! contains no `unsafe` and no FFI, so it compiles unchanged on the host and on
//! `aarch64-linux-android`/iOS with no cfg gymnastics. Each platform shell keeps
//! its own FFI boundary (JNI exports, raw-window handling) and reuses this crate
//! rather than duplicating the plumbing.

mod app_tree;
mod ffi_support;

pub use app_tree::{AppTree, new_boxed_app};
pub use ffi_support::{guard, logical_size, sanitize_scale};
