//! `frust-native-widgets`: render REAL platform widgets (Android `View`s /
//! UIKit views) from pure Rust — `native_button("Save")`, `native_switch(...)`
//! etc. compose the framework's `platform_view`
//! slots for placement, while this plugin creates and mutates the actual
//! native views through direct, same-thread FFI: `with_jni_env` + hand-curated
//! JNI bindings on Android, `objc2-ui-kit` on iOS (the factory itself a Rust
//! `define_class!` class — zero Swift). This crate holds the
//! retained-handle [`registry`] every control's create/update/dispose path is
//! built over, the `runtime` those paths dispatch through, the six `controls`
//! (`Button`, `Label`, `Switch`, `Slider`, `ProgressBar`, `Image`) that
//! runtime serves, the typed `events` vocabulary their listeners decode into,
//! the app-facing `api` builders, and both platform arms' factory glue. Each
//! control now carries **both** platform halves — a
//! `#[cfg(target_os = "android")] mod platform` and a
//! `#[cfg(target_os = "ios")] mod platform`, side by side in the same file,
//! executing the same shared setter plan and
//! reporting a tap/toggle/drag back to the app through the same event
//! dispatch on both platforms (`crate::apple::events`'s Rust target-action
//! object, mirroring Android's shared listener). The theme ladder's Apple
//! arm is also in: L1 (`crate::apple::theme`) pins brightness via
//! `overrideUserInterfaceStyle` at control-creation time; L2 applies the
//! same folded `Props` tokens through typed `objc2-ui-kit`/`CALayer`
//! setters, including a themed background's corner radius; L3
//! (`crate::apple::fonts`) resolves the embedded Glyph faces to a real
//! `CTFont` via CoreText, degrading to the system font (one logged warning)
//! on any resolution failure.
//!
//! # One factory, one listener, N controls
//!
//! Adding a control never adds Kotlin or Swift — and
//! that holds for **your** components too: `NativeComponent` is the public
//! trait a plugin author implements to drive a native view (or view
//! hierarchy) from pure Rust, registered with `register_component` and served
//! by the very same runtime, factory and listener as the six built-in
//! controls (see the `component` module's own doc for the lifecycle contract;
//! it ships with the `frust-api` feature, like the builders above).
//!
//! Two limits that doc states in full, repeated here because they decide
//! whether the trait is for you at all. **An app crate cannot implement it
//! today**: `create` has to name `jni::objects::JObject` on Android and
//! `objc2-ui-kit`'s classes on iOS *in the implementing crate*, and this
//! plugin re-exports neither FFI crate, so the practical audience today is
//! plugin authors, not app authors (the only implementor here is this crate's
//! own non-default `demo-components` composite). And **a component is
//! display-only**: no production path attaches a listener to a view a
//! component built — its root as much as its children — so overriding
//! `NativeComponent::on_event` has no effect in this build, a deliberately
//! deferred Phase 4 gap. Deliberately *not* "can never fire": the dispatch
//! half is real and routes on the slot id alone, so a listener built with a
//! **fabricated** id that happens to name a live component's slot still lands
//! in the trait method — a misroute, not a route (see that method's own doc).
//!
//! Every control is a Rust
//! `NativeWidget` impl registered under a kind string in the plugin-internal
//! `runtime`, and the platform side is fixed forever at ONE generic factory
//! class plus ONE generic listener class (Android:
//! `plugins/native-widgets/platform/android/`, driven by this crate's four
//! JNI exports; iOS: a Rust `define_class!` factory registered straight into
//! the Objective-C runtime, `crate::apple::factory` — zero Swift, and target
//! -action instead of a listener class). Which control a `platform_view` slot
//! means travels in that slot's `params_json`, under two reserved keys the api
//! layer injects — the same payload that carries the differ's slot id across a
//! factory contract that does not pass it.
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-camera`](../frust_camera/index.html) and
//! [`frust-secure-storage`](../frust_secure_storage/index.html), this is a
//! **platform plugin** (see `docs/ARCHITECTURE.md`'s Module Structure): it
//! depends on `frust-plugin` plus FFI crates only, and carries **no other
//! `frust-*` framework dependency**. An app adds this crate to its own
//! `Cargo.toml` alongside `frust`; the facade does not depend on or
//! re-export it.
//!
//! # Placement doctrine
//!
//! Placement is **static-first**: app bars, bottom bars, and full-body
//! surfaces have no desync by construction, while a *scroll-hosted* native
//! widget is a documented, degraded tier (an earlier device measurement of a
//! scrolled native camera preview found cross-pipeline desync only while a
//! slot's rect moves relative to frust content). Nothing in v1
//! may *require* hosting a native widget inside a scroller; the headline use
//! case is a high-rate native surface (a live chart, a streaming dashboard)
//! updating at panel rate while frust's own frame loop idles.
//!
//! # Native-widget events bypass `RenderRoot::event`
//!
//! A native control's interaction is entirely platform-owned: a tap fires
//! the platform's own listener (Android's `View.OnClickListener`, iOS
//! target-action), which this crate's plugin-private JNI/ObjC exports
//! deliver straight into a registered Rust callback — never through
//! `frust-core`'s `EventCtx`. That means every `frust-core`/`frust-widgets`
//! interaction convention (`docs/CODE_STANDARDS.md`'s Interaction Semantics:
//! capture, focus, fire-on-up-inside, `Cancel`-never-mutates-state,
//! `reduce_motion`/state-layer conventions) simply does not apply to a
//! native control — this is a defining property of hosting real platform
//! widgets, not a gap to close. The app-facing API wraps a
//! native event straight into a signal write, which is what wakes exactly
//! one frust frame.

// The app-facing builders: default-on so an app that just adds this
// crate to its `Cargo.toml` gets `native_button`/`native_label`/etc. for
// free, but fully feature-gated — see `api`'s module doc and `Cargo.toml`'s
// comment on why the crate stays a pure platform plugin without it.
#[cfg(feature = "frust-api")]
pub mod api;
// Flat re-export at the crate root, mirroring the `frust` facade's own
// flatten-every-widget convention — `native_button(...)` rather than
// `api::native_button(...)`.
#[cfg(feature = "frust-api")]
pub use api::*;

// The public `NativeComponent` trait — see its module doc.
// Behind the same `frust-api` gate as the builders above, for one reason: a
// component is only *mountable* through a `platform_view` slot, which is
// facade glue this crate only has with the feature on. With it off the crate
// is a bare platform plugin (`frust-plugin` + FFI crates, the charter line
// `cargo tree -p frust-native-widgets --no-default-features -e normal`
// checks) and there would be nothing to hand a component to.
#[cfg(feature = "frust-api")]
pub mod component;
// Flat re-export, same convention as `api` above.
#[cfg(feature = "frust-api")]
pub use component::{
    ComponentCtx, NativeChild, NativeComponent, NativeEvent, NativeRoot, register_component,
};

// The demo composite — ONE `NativeComponent` owning a real
// native subtree, behind the NON-default `demo-components` feature (which
// enables `frust-api` above, since a component is only mountable through that
// facade glue). It lives in this crate rather than in an example app because an
// app crate cannot implement the trait without raw `jni`/`objc2-ui-kit` deps of
// its own; see the module's own doc for what that does and does not prove.
#[cfg(feature = "demo-components")]
pub mod demo;
// Flat re-export, same convention as `api`/`component` above.
#[cfg(feature = "demo-components")]
pub use demo::{
    DEMO_CARD_CHILDREN, DEMO_CARD_HEIGHT, DEMO_CARD_KIND, DEMO_CARD_WIDTH, DemoCard, DemoCardProps,
    DemoCardState, register_demo_components,
};

#[cfg(target_os = "android")]
mod android;
// The Apple arm: ONE Rust `define_class!` factory class conforming to
// the embedding's `FrustPlatformViewFactory` protocol — no Swift, no exports.
// iOS only, not `target_vendor = "apple"`: see `Cargo.toml`'s comment on why
// (UIKit doesn't exist on macOS).
#[cfg(target_os = "ios")]
mod apple;
// The six v1 controls. Compiled on every target on purpose: each control's
// props/decode/diff half is platform-agnostic and host-tested, and only its
// `NativeWidget` impl (the JNI half) is `#[cfg(target_os = "android")]` —
// which is also why the modules live here rather than under `android/`. The
// `allow` matches `runtime`'s below: on a non-Android host the whole
// props/plan surface has no caller outside the tests.
#[allow(dead_code)]
mod controls;
// The typed event vocabulary (`EventPayload`) and the kind/detail codec every
// interactive control's listener decodes through — platform-agnostic and
// host-tested for the same reason `controls` is (see that module's `allow`).
#[allow(dead_code)]
mod events;
mod registry;
// The runtime's surface is consumed by the platform arms — this crate's JNI
// exports, the Apple `define_class!` factory, the six controls and their
// listeners — plus its own host tests, which a plain (non-test) build does
// not count. On a non-mobile host none of those arms compile, so much of the
// surface is legitimately uncalled there; the attribute stays for that host
// build rather than growing per-item `allow`s.
#[allow(dead_code)]
mod runtime;

pub use registry::{Registry, SlotId};

#[cfg(target_os = "android")]
pub use registry::android::AndroidHandle;
// iOS only — see `Cargo.toml`'s comment on why this crate's Apple arm gates
// on `target_os = "ios"` rather than `target_vendor = "apple"` (no macOS/
// AppKit backend; UIKit doesn't exist there).
#[cfg(target_os = "ios")]
pub use registry::apple::AppleHandle;

/// Errors from a native-widgets operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant (e.g. distinguishing [`Self::PlatformNotInitialized`] from a
/// generic [`Self::Platform`] failure) rather than only displaying it.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum NativeWidgetError {
    /// This platform has no native-widgets backend (desktop preview, wasm),
    /// or an Android scaffold predating `nativeInitPlatform` — the same
    /// fail-soft contract every other platform plugin uses
    /// (`docs/CODE_STANDARDS.md`'s Plugin Conventions), never a panic.
    #[error("native-widgets platform not initialized")]
    PlatformNotInitialized,

    /// A slot's `params_json` could not be read: the reserved identity keys
    /// are missing, a required field is absent or malformed, or the params
    /// name a different control than the live instance. Distinguishable from
    /// [`Self::Platform`] because it is an api-layer/runtime contract
    /// violation, never a platform failure.
    #[error("native-widgets params error: {0}")]
    Params(String),

    /// No `NativeWidget` is registered under the control kind a slot's params
    /// name — a control the app's build never registered (or a params payload
    /// from a different plugin version).
    #[error("native-widgets: no control registered as '{0}'")]
    UnknownControl(String),

    /// A backend-specific failure not covered by a more specific variant
    /// (a JNI error, an unexpected ObjC runtime failure).
    #[error("native-widgets platform error: {0}")]
    Platform(String),
}

/// The un-contexted fallback conversion, so a JNI error can ride `?` through
/// a helper that has no operation name to attach (the local-frame wrapper is
/// the one such path). **Prefer `NativeCtx::run_jni`**, which names the
/// failing operation and converts a pending Java exception into a message —
/// this impl exists for the plumbing that cannot.
#[cfg(target_os = "android")]
impl From<jni::errors::Error> for NativeWidgetError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Platform(format!("jni: {error}"))
    }
}
