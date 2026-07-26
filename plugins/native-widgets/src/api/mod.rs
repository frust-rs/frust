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
//! # No public `NativeWidget` trait
//!
//! Every builder in [`builders`] is a plain data struct implementing
//! [`frust_core::Component`] (`frust-core`'s retained-local-state seam) —
//! never the crate's own `pub(crate)` [`crate::runtime::NativeWidget`] trait,
//! which stays internal (Phase 3 decides its public form,
//! `RESEARCH-NATIVE-COMPONENT.md`'s "What v1 deliberately excludes").
//!
//! # One `platform_view` slot per control (PLAN 3.1)
//!
//! Each builder composes exactly one `frust::platform_view` slot resolving to
//! this crate's one Android factory (`dev.frust.FrustNativeControlFactory`)
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
mod signals;
mod theme;

pub use builders::{
    NativeButtonView, NativeImageFit, NativeImageView, NativeLabelView, NativeProgressView,
    NativeSliderView, NativeSwitchView, native_button, native_image, native_label, native_progress,
    native_slider, native_switch,
};
