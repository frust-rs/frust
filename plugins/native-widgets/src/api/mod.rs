//! The app-facing `api` feature (native-widgets Phase 1, p1-06): builders
//! returning `impl View<State>` compositions over the platform-agnostic
//! `controls`/`runtime`/`events` machinery `crate` already carries — the
//! facade-glue half PLAN 3.1 sanctions depending on `frust`.
//!
//! # Feature-gated, not the crate's default shape
//!
//! Everything under this module compiles ONLY behind the default-on
//! `frust-api` feature — `cargo check -p frust-native-widgets
//! --no-default-features` must still hold the platform-plugin charter line
//! (`frust-plugin` + FFI crates only, `docs/ARCHITECTURE.md`'s Module
//! Structure): no `frust`/`frust-core` dependency reaches the crate at all
//! with the feature off. See `Cargo.toml`'s comment on why this crate keeps
//! `frust-core` alongside `frust` under the same gate (a downstream `View`
//! impl needs `BuildCtx`/`ChangeFlags`, which the `frust` facade does not
//! re-export).
//!
//! # The internal `NativeWidget` trait is still not the public one
//!
//! Every builder in [`builders`] is a plain data struct implementing
//! [`frust_core::Component`] (`frust-core`'s retained-local-state seam) —
//! never the crate's own `pub(crate)` [`crate::runtime::NativeWidget`] trait,
//! which stays internal permanently (p3-01's public-surface decision).
//!
//! Phase 3 answered what the *public* form is, and it is a different trait:
//! [`crate::component::NativeComponent`], with `&self` methods, already-typed
//! `Props` the app constructs directly, no `decode_props` step and no `Result`
//! returns. The six builders here keep riding the internal trait's wire
//! (`params_json` in, `EventPayload` callbacks out) unchanged — bridging the
//! public trait onto the same runtime (`crate::component::Bridge`) is what let
//! that stay true.
//!
//! # Two builder families, one slot shape
//!
//! The six built-in controls above are one family; [`native_component`] is
//! the **generic** one, mounting any registered
//! [`NativeComponent`](crate::component::NativeComponent) (another plugin's
//! included — an app crate cannot implement one; see that trait's own doc)
//! into the same single `platform_view` slot, reusing the six's
//! own factory constant, slot counter, sizing rule and refusal placeholder.
//! It is what closes *define → register → mount*; the six are deliberately
//! not rewritten to route through it (see `src/api/mount.rs`'s module doc).
//!
//! # One `platform_view` slot per control (PLAN 3.1)
//!
//! Each builder composes exactly one `frust::platform_view` slot resolving to
//! this crate's one Android factory
//! (`dev.frust.nativewidgets.FrustNativeControlFactory`)
//! — N controls = N slots, within the differ's design envelope (a
//! shared-container optimization is future work).
//!
//! # Theme ladder L2 (p1-07)
//!
//! Each builder's `Component::build` reads the active theme
//! (`use_context::<Theme>()`) and folds it, via [`theme::resolve`], into the
//! same `params_json` body every other property already rides — see
//! [`theme`]'s module doc for the mapping table and `crate::android::theme`
//! for L1 (the night-qualified `Context` control creation builds against).

mod builders;
mod mount;
mod signals;
mod theme;

pub use builders::{
    NativeButtonView, NativeImageFit, NativeImageView, NativeLabelView, NativeProgressView,
    NativeSliderView, NativeSwitchView, native_button, native_image, native_label, native_progress,
    native_slider, native_switch,
};
pub use mount::{NativeComponentView, native_component};

/// Make sure this build's platform factory exists before the host can look it
/// up — **optional**: every builder already does this for you.
///
/// On iOS the factory a `platform_view` slot resolves to is a Rust
/// `define_class!` Objective-C class (`crate::apple::factory` — zero Swift),
/// and objc2 registers such a class with the Objective-C runtime **lazily**,
/// on the first call from live Rust code. Nothing on the platform side can
/// trigger that: `FrustViewHost` only ever asks for the class *by name*
/// (`NSClassFromString`), which returns nil for a class that was never
/// registered. Every builder in this module therefore forces registration on
/// the same rebuild that publishes its slot — a whole frame before the host's
/// post-frame command poll can resolve it — so an app that just calls
/// `native_button(...)` needs nothing from this function.
///
/// Call it anyway if you want registration to happen at a moment you choose
/// (app startup, say) rather than at first use; it is idempotent, cheap after
/// the first call, and a no-op on every non-iOS target — Android's factory is
/// a Kotlin class that exists whether or not Rust has run.
pub fn ensure_native_factory_registered() {
    crate::runtime::ensure_platform_factory();
}

/// The number of native controls this crate's internal runtime currently
/// retains — the leak bar `registry::Registry::live_count`'s own doc comment
/// describes ("the number the leak bar every create/dispose cycle must
/// return to `0`"), surfaced app-side for exactly one reason: task
/// p1-11's device-gate harness (a mount/unmount cycler plus a 50-slot stress
/// toggle, `examples/glyph-catalog/src/pages/native_widgets.rs`'s GATE
/// HARNESS section) needs an in-app readout to prove the p1-09 teardown-retire
/// path disposes promptly rather than waiting out the differ's
/// missing-streak backstop (`crate::registry`'s module doc's Idle-deferred
/// dispose finding).
///
/// **Diagnostics/gate accessor, not a supported production API.** It leaks
/// no registry type, no `NativeWidget` trait, and no handle — just a plain
/// count. Phase 3's public-surface decision (p3-01) kept it exactly as it is,
/// for exactly that reason: it says nothing about the runtime's shape, so
/// nothing about it constrains the public
/// [`NativeComponent`](crate::component::NativeComponent) surface. It still
/// carries no compatibility promise — a future release may narrow, rename or
/// remove it outright, and a component's live count is included in the number.
/// Don't build product behavior on it.
///
/// Returns `0` on a re-entrant call (the runtime's own thread-local is
/// already borrowed on this thread — `crate::runtime::with_runtime`'s
/// documented re-entrancy tolerance) as well as on a platform with no live
/// runtime at all; either way indistinguishable from "nothing is mounted"
/// for this accessor's diagnostic purpose.
pub fn live_slot_count() -> usize {
    crate::runtime::with_runtime(|runtime| runtime.live_count()).unwrap_or(0)
}
