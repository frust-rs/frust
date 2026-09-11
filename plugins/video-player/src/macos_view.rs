//! Placeholder for the macOS native video view: the layer-backed `NSView`
//! subclass hosting an `AVPlayerLayer`, plus the factory the desktop
//! platform-view registry resolves under [`crate::VIEW_TYPE`] — the same
//! bare name iOS publishes to the ObjC runtime, since an app names one
//! factory for both.
//!
//! Written in Rust with `objc2`'s `define_class!`, reaching its session's
//! player through [`crate::apple::player_for`]. The hosted view is an opaque
//! native sibling (the platform-view Mode A contract), so the frust slot
//! paints nothing beneath it.

// The registration hook below has no caller until the factory it registers
// exists.
#![allow(dead_code)]

/// Register the macOS video-view factory with the desktop platform-view
/// registry, once.
///
/// A no-op placeholder today. The real implementation registers lazily and
/// is idempotent, so the session open path can call it unconditionally.
pub(crate) fn ensure_registered() {}
