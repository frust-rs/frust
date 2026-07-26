//! `frust-native-widgets`: render REAL platform widgets (Android `View`s /
//! UIKit views) from pure Rust — `native_button("Save")`, `native_switch(...)`
//! etc. (Phase 3's app-facing API) compose the framework's `platform_view`
//! slots for placement, while this plugin creates and mutates the actual
//! native views through direct, same-thread FFI: `with_jni_env` + hand-curated
//! JNI bindings on Android, `objc2-ui-kit` on iOS (the factory itself a Rust
//! `define_class!` class — zero Swift, Phase 0 spike 2, GO). See
//! `workflow/plans/features/frust-native-widgets/PLAN.md` for the full plan;
//! this crate lands incrementally across that plan's tasks — today it is
//! the crate skeleton plus the retained-handle [`registry`] every later
//! phase's create/update/dispose path is built over.
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
//! widget is a documented, degraded tier (the camera scroll-sync spike
//! measured cross-pipeline desync only while a slot's rect moves relative to
//! frust content — `research/RESEARCH-PLACEMENT.md` §1/§3). Nothing in v1
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
//! widgets, not a gap to close. The app-facing API (Phase 3) wraps a
//! native event straight into a signal write, which is what wakes exactly
//! one frust frame.

mod registry;

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

    /// A backend-specific failure not covered by a more specific variant
    /// (a JNI error, an unexpected ObjC runtime failure).
    #[error("native-widgets platform error: {0}")]
    Platform(String),
}
