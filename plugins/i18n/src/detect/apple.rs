//! The iOS detection backend — `NSLocale.preferredLanguages`, plain
//! Foundation via `objc2-foundation`.
//!
//! `#[cfg(target_os = "ios")]` only — **not** `target_vendor = "apple"`:
//! macOS shares [`crate::detect::desktop`] (`sys-locale`) instead, per this
//! crate's own `Cargo.toml` design note (the wider `target_vendor = "apple"`
//! dependency block just lands `objc2`/`objc2-foundation` in the macOS
//! dependency graph without this module ever being compiled there).
//!
//! No platform-handle init needed here — unlike [`crate::detect::android`],
//! `objc2` reaches the ObjC runtime globally
//! (`docs/PLUGINS_ARCHITECTURE.md`'s Data Flow: "Apple needs no init step").
//!
//! `NSLocale::preferredLanguages()` is a plain safe binding in the pinned
//! `objc2-foundation 0.3` line as resolved by this workspace's lockfile
//! (`0.3.2` — `0.3.1` marked the same method `unsafe fn`; this module holds
//! no `unsafe` block because of that resolved version, not because the
//! method is inherently safe on every `0.3.x` patch).

use objc2_foundation::NSLocale;

/// `NSLocale.preferredLanguages`, most-preferred first — Apple's own
/// preference list is already BCP-47-shaped closely enough that
/// `unic_langid` parses it directly; anything it doesn't is skipped by
/// `detect::mod`'s [`super::parse_tags`], not filtered here.
///
/// `to_vec()` (rather than `iter()`) is deliberate: it needs no
/// `NSEnumerator` feature beyond the `NSLocale`/`NSArray`/`NSString`
/// already enabled in this crate's `Cargo.toml`.
pub(crate) fn raw_locale_tags() -> Vec<String> {
    NSLocale::preferredLanguages()
        .to_vec()
        .iter()
        .map(|tag| tag.to_string())
        .collect()
}
