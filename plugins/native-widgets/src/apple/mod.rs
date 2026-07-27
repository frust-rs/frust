//! The Apple (iOS) backend: ONE Rust `define_class!` Objective-C factory
//! class, and nothing else — the whole platform surface this plugin will ever
//! need on Apple, however many controls [`crate::runtime`] grows.
//!
//! The Android arm's mirror image, minus the glue. Android needs two Kotlin
//! classes and four JNI exports because a Java factory has to call *into*
//! Rust; on iOS the factory **is** Rust ([`factory`]), registered directly
//! with the Objective-C runtime and looked up by name — **zero Swift** (PLAN
//! Phase 0 spike 2, GO in a signed release build). Read [`factory`]'s module
//! doc for the frozen protocol contract, the two load-bearing name pins, the
//! non-nil failure contract, and why no `ios_exports!`/`#[used]` static is
//! needed here.
//!
//! # iOS only, not `target_vendor = "apple"`
//!
//! Unlike `plugins/camera`/`plugins/shared-preferences`/
//! `plugins/secure-storage`, this plugin's Apple surface is UIKit, which does
//! not exist on macOS — gating on `target_vendor = "apple"` makes a macOS
//! desktop-preview build try to link the `UIKit` framework and fail. See
//! `Cargo.toml`'s comment on the same gate.
//!
//! # Threading
//!
//! Every entry point runs on the platform main thread: the host drives the
//! factory from its post-frame command poll (`FrustViewHost`, itself driven
//! from `FrustViewController`'s `CADisplayLink`), and a `UIControl`'s
//! target-action fires there too. Unlike the Android arm, that is not a
//! `debug_assert` — the factory class is `#[thread_kind = MainThreadOnly]`, so
//! its methods can only be entered with a [`MainThreadMarker`] in hand, and
//! [`NativeCtx`] carries that proof into every control call (PLAN 2.1).
//!
//! [`MainThreadMarker`]: objc2::MainThreadMarker

mod ctx;
mod factory;

pub(crate) use ctx::NativeCtx;
pub(crate) use factory::ensure_registered;

use crate::runtime::NativeRuntime;

/// A control's retained native reference — on Apple that is exactly the
/// registry's [`AppleHandle`], an ARC-managed `Retained<UIView>` behind a
/// [`MainThreadMarker`](objc2::MainThreadMarker) gate.
///
/// No paired-delete discipline is needed on this arm (`Retained`'s own `Drop`
/// releases the object), which is the one structural difference from
/// `crate::android`'s `NativeView` and its `extra` global-ref list.
///
/// [`AppleHandle`]: crate::registry::apple::AppleHandle
pub(crate) type NativeView = crate::registry::apple::AppleHandle;

/// Register every control kind this backend serves, once, when the thread's
/// runtime is first touched (`crate::runtime`'s `seeded_runtime`) — the Apple
/// mirror of `crate::android::register_controls`.
///
/// **Empty until p2-02.** The six controls live in the shared
/// `crate::controls`, where the props/plan half is platform-neutral and
/// host-tested and only the platform half is a per-target `mod platform`
/// inside each control file; the Apple `platform` arms (and therefore the
/// `NativeWidget` impls this function would register) land in p2-02. Until
/// then an iOS `create` reports
/// [`NativeWidgetError::UnknownControl`](crate::NativeWidgetError::UnknownControl)
/// and the factory returns its documented dead-slot placeholder — visible in
/// the log, not a crash.
///
/// Registration stays explicit and central by design: `inventory`-style
/// link-time discovery is banned here (RESEARCH-NATIVE-COMPONENT §Open
/// questions), which matters even more on this arm, where `lto = "fat"` +
/// `strip = "symbols"` is the shipping configuration.
pub(crate) fn register_controls(_runtime: &mut NativeRuntime) {}
