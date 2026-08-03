//! The iOS [`Backend`] — `UISelectionFeedbackGenerator`/
//! `UIImpactFeedbackGenerator`/`UINotificationFeedbackGenerator` via
//! `objc2-ui-kit`.
//!
//! `#[cfg(target_os = "ios")]` only — **not** `target_vendor = "apple"`:
//! these are UIKit classes, which don't exist on macOS (no AppKit
//! equivalent this crate targets in v1 — see [`crate::desktop`]'s module
//! doc). Matches `frust-clipboard`'s `apple.rs` rationale for the same
//! `target_os`/`target_vendor` split.
//!
//! # Main-thread dispatch
//!
//! Every `UI*FeedbackGenerator` class is UIKit's `MainThreadOnly` — both the
//! `objc2-ui-kit` binding (`#[thread_kind = MainThreadOnly]`, requiring a
//! [`MainThreadMarker`] to construct one) and Apple's own documentation agree
//! generator construction and every trigger method must happen on the main
//! thread. Unlike [`crate::android`]'s `Vibrator.vibrate` (which carries no
//! such requirement and is called inline), this backend cannot simply call
//! through from whatever thread `Haptics::perform` runs on — so
//! [`Backend::perform`] dispatches the entire construct-and-trigger sequence
//! onto `dispatch_get_main_queue()` **asynchronously**
//! (`DispatchQueue::exec_async`), returning `Ok(())` immediately without
//! waiting for it to run. This is deliberate, not merely convenient: a
//! *synchronous* main-thread bounce (`exec_sync`, or `dispatch2::run_on_main`'s
//! blocking fallback for a non-main caller) would violate this crate's
//! fire-and-forget, never-block-the-caller contract if `Haptics::perform`
//! is ever called from a background thread — `exec_async` never blocks
//! regardless of the calling thread, matching the Steps section of this
//! crate's originating task ("iOS dispatches async to main").
//!
//! `exec_async`'s closure runs *after* `perform` has already returned, so a
//! caller cannot observe the generator's own construction/trigger failing —
//! there is nothing to report back on this platform's path, matching
//! [`Backend::perform`]'s doc.
//!
//! # `unsafe`
//!
//! None. Every `UI*FeedbackGenerator` method this backend calls
//! (`selectionChanged`/`impactOccurred`/`notificationOccurred:`/`prepare`,
//! and the `new(mtm)`/`alloc(mtm)` constructors) is a plain safe `pub fn` in
//! `objc2-ui-kit`'s generated bindings — unlike `frust-clipboard`'s
//! `UIPasteboard::string`/`setString:`, nothing here is marked `unsafe`
//! except the one-line proof that a `dispatch_get_main_queue()` callback
//! runs on the actual OS main thread (see [`Backend::perform`]).
//!
//! # `initWithStyle:` is not an Apple deprecation
//!
//! `UIImpactFeedbackGenerator::initWithStyle` is marked `#[deprecated]` by
//! `objc2-ui-kit`'s header-translator — this is that tool's own convention of
//! flagging every hand-written Objective-C initializer in favor of the
//! `new(mtm)`/`alloc(mtm)` convenience pair it generates alongside, not an
//! Apple SDK deprecation (`-initWithStyle:` remains Apple's own documented,
//! current way to give an impact generator a style without attaching it to a
//! `UIView`, which is the only other constructor UIKit offers one). The
//! `#[allow(deprecated)]` on [`impact`] is scoped to the one function that
//! needs it, mirroring `plugins/camera/src/apple.rs`'s
//! `set_legacy_video_orientation` precedent for the same situation.

use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_ui_kit::{
    UIImpactFeedbackGenerator, UIImpactFeedbackStyle, UINotificationFeedbackGenerator,
    UINotificationFeedbackType, UISelectionFeedbackGenerator,
};

use crate::{Backend, HapticEffect, HapticsError};

/// The iOS backend. Stateless — every operation constructs a fresh generator
/// (Apple's own guidance: generators are cheap, single-use objects, not
/// long-lived singletons like `UIPasteboard.generalPasteboard`).
pub(crate) struct AppleHaptics;

impl Backend for AppleHaptics {
    fn perform(&self, effect: HapticEffect) -> Result<(), HapticsError> {
        // Fire-and-forget: dispatch async to the main queue and return
        // immediately — never block the caller (module doc's *Main-thread
        // dispatch*). `HapticEffect` is `Copy`, satisfying `exec_async`'s
        // `Send + 'static` bound with nothing to borrow.
        DispatchQueue::main().exec_async(move || {
            // SAFETY: this closure is submitted to
            // `dispatch_get_main_queue()`, which always executes its blocks
            // on the actual OS main thread (module doc's *Main-thread
            // dispatch*) — so constructing a `MainThreadMarker` here without
            // re-checking is sound.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            perform_on_main(mtm, effect);
        });
        Ok(())
    }
}

/// The exhaustive match (no wildcard arm) that is this backend's half of the
/// crate-wide compile-level routing guarantee (the crate doc's *Backends*
/// section) — runs on the main queue (see [`Backend::perform`]).
fn perform_on_main(mtm: MainThreadMarker, effect: HapticEffect) {
    match effect {
        HapticEffect::SelectionClick => {
            let generator = UISelectionFeedbackGenerator::new(mtm);
            generator.prepare();
            generator.selectionChanged();
        }
        HapticEffect::ImpactLight => impact(mtm, UIImpactFeedbackStyle::Light),
        HapticEffect::ImpactMedium => impact(mtm, UIImpactFeedbackStyle::Medium),
        HapticEffect::ImpactHeavy => impact(mtm, UIImpactFeedbackStyle::Heavy),
        HapticEffect::Success => notification(mtm, UINotificationFeedbackType::Success),
        HapticEffect::Warning => notification(mtm, UINotificationFeedbackType::Warning),
        HapticEffect::Error => notification(mtm, UINotificationFeedbackType::Error),
    }
}

/// `UIImpactFeedbackGenerator(style:)` + `impactOccurred()` — see the module
/// doc's *`initWithStyle:` is not an Apple deprecation* for why this needs
/// `#[allow(deprecated)]`.
#[allow(deprecated)]
fn impact(mtm: MainThreadMarker, style: UIImpactFeedbackStyle) {
    let generator =
        UIImpactFeedbackGenerator::initWithStyle(UIImpactFeedbackGenerator::alloc(mtm), style);
    generator.prepare();
    generator.impactOccurred();
}

/// `UINotificationFeedbackGenerator()` + `notificationOccurred(_:)`.
fn notification(mtm: MainThreadMarker, kind: UINotificationFeedbackType) {
    let generator = UINotificationFeedbackGenerator::new(mtm);
    generator.prepare();
    generator.notificationOccurred(kind);
}
