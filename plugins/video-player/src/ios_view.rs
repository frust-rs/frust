//! Placeholder for the iOS native video view: the `UIView` subclass hosting
//! an `AVPlayerLayer`, plus the `@objc(VideoPlayerViewFactory)` factory
//! `frust::platform_view` resolves through `NSClassFromString` (see
//! [`crate::VIEW_TYPE`]).
//!
//! Written in Rust with `objc2`'s `define_class!` — no Swift glue, and no C
//! export: the factory reaches its session's player through
//! [`crate::apple::player_for`].

// The registration hook below has no caller until the factory it registers
// exists.
#![allow(dead_code)]

/// Register the iOS video-view factory with the ObjC runtime, once.
///
/// A no-op placeholder today. The real implementation defines the factory
/// class lazily on first call (a `define_class!` class must be realized
/// before `NSClassFromString` can find it) and is idempotent, so the session
/// open path can call it unconditionally.
pub(crate) fn ensure_registered() {}
