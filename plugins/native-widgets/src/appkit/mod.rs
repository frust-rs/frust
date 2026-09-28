//! The macOS backend: ONE Rust `frust_plugin::desktop::DesktopViewFactory`
//! plus ONE `define_class!` target-action class, and nothing else — the whole
//! platform surface this plugin needs on macOS, however many controls
//! [`crate::runtime`] grows.
//!
//! The third arm, beside Android (`crate::android`) and iOS (`crate::apple`).
//! Where the iOS factory is an ObjC class the embedding looks up by name, the
//! macOS one is a plain Rust value registered into `frust_plugin::desktop`'s
//! process-global `view_type -> factory` table ([`factory`]), which
//! `crates/frust-shell-macos`' desktop Mode-A host reads: a hosted control is
//! an opaque AppKit sibling parented above winit's content view, positioned in
//! logical points from the differ's rect. Read [`factory`]'s module doc for
//! the retain accounting, the failure contract, and theme ladder L1
//! (`NSAppearance`), applied inline on every create and update; [`events`]'s
//! for the target-action attach/detach and per-slot retention.
//!
//! # Its own module, not a widened `apple` gate
//!
//! UIKit and AppKit share the ARC ownership model and nothing else a control
//! touches (`UIButton` vs `NSButton`, target-action per control event vs one
//! action per control), so the two Apple arms are separate modules over the
//! same runtime; `Cargo.toml` gives each its own target block. No Mac Catalyst.
//!
//! # Input on a desktop host
//!
//! The host reads a slot's `interactive` flag and deliberately ignores it:
//! AppKit routes a click to the topmost view under the pointer through the
//! responder chain, so a hosted `NSControl` receives its own clicks with no
//! input shield or hit-test forwarding from frust. The flip side (an accepted
//! Mode-A limitation) is that frust chrome cannot draw over a hosted control.
//!
//! # Threading
//!
//! Every entry point runs on the platform main thread: the host drives the
//! factory from its post-frame platform-view hook on winit's event-loop
//! thread, and AppKit sends target-action there too. The target class is
//! `#[thread_kind = MainThreadOnly]` and the factory checks
//! [`MainThreadMarker::new`](objc2::MainThreadMarker::new) once per call;
//! [`NativeCtx`] carries that proof into every control call.

mod ctx;
mod events;
pub(crate) mod factory;
// Theme ladder L1: `NSAppearance` pure helpers (`apply_brightness`,
// `brightness_is_dark`); pinned per hosted view on create AND every update
// from inside `factory` itself (`factory`'s own module doc). L2/L3 need
// nothing here — their setters are `crate::controls::platform`'s macOS half,
// and L3's CoreText half is the shared `crate::coretext`.
mod theme;

pub(crate) use ctx::NativeCtx;
pub(crate) use events::FrustNativeControlTarget;
pub(crate) use factory::ensure_registered;

use crate::controls::{button, image, label, progress, slider, switch};
use crate::runtime::NativeRuntime;

/// A control's retained native reference — on macOS exactly the registry's
/// [`AppKitHandle`], an ARC-managed `Retained<NSView>` behind a
/// [`MainThreadMarker`](objc2::MainThreadMarker) gate (the AppKit twin of
/// `crate::apple`'s alias of the same name).
///
/// [`AppKitHandle`]: crate::registry::appkit::AppKitHandle
pub(crate) type NativeView = crate::registry::appkit::AppKitHandle;

/// Register every control kind this backend serves, once, when the thread's
/// runtime is first touched (`crate::runtime`'s `seeded_runtime`) — the macOS
/// mirror of `crate::apple::register_controls`, by the same shared `KIND`
/// consts (never a literal).
///
/// All six v1 controls, in the Android/iOS order, **line for line** — so the
/// three arms register the same kind table (`crate::controls::tests`'
/// `the_six_control_kinds_are_the_same_strings_all_three_platform_arms_register`
/// is the host-visible half of that pin). A slot naming a kind outside it
/// gets [`factory`]'s empty dead-slot view (`NativeWidgetError::UnknownControl`,
/// logged once per create).
///
/// Registration stays explicit and central for the reason
/// `crate::apple::register_controls` gives (no link-time discovery).
pub(crate) fn register_controls(runtime: &mut NativeRuntime) {
    runtime.register::<button::Button>(button::KIND);
    runtime.register::<label::Label>(label::KIND);
    runtime.register::<switch::Switch>(switch::KIND);
    runtime.register::<slider::Slider>(slider::KIND);
    runtime.register::<progress::Progress>(progress::KIND);
    runtime.register::<image::Image>(image::KIND);
}
