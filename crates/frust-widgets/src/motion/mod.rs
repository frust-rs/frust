//! Motion module (glyph-design-system, task 12): declarative single-child
//! animation wrappers built over `frust-core`'s `anim` vocabulary
//! ([`frust_core::AnimationController`]/[`frust_core::Curve`]/
//! [`frust_core::Tween`]/[`frust_core::Spring`]) and the paint-only
//! `push_layer`/`push_transform` compositing primitives
//! ([`frust_core::PaintScene`]).
//!
//! # Module map
//!
//! Every motion module is pre-declared here so a later task adds its code to
//! its own file and never edits this module list (mirrors `nav/mod.rs`'s
//! module-map precedent):
//!
//! * [`animated`] (task 12, this task) — [`AnimatedOpacity`]/[`AnimatedScale`]:
//!   declarative single-child implicit-animation wrappers. A target-value
//!   change retargets smoothly (no jump), timed from the theme's
//!   [`frust_theme::MotionScheme`] by default, or an explicit
//!   [`crate::Timing`] override.
//! * [`switcher`] (task 16) — a doc-only stub today; a later task fills this
//!   in with a cross-fading child-switch wrapper (Flutter's
//!   `AnimatedSwitcher`).
//! * [`patterns`] (task 19) — a doc-only stub today; a later task fills this
//!   in with higher-level motion patterns (e.g. staggered reveals) composed
//!   over [`animated`]/[`switcher`].
//!
//! # Wholesale facade re-export
//!
//! Unlike the baseline/catalog widgets' flat per-type re-export lists, the
//! `frust` facade re-exports this module **wholesale**
//! (`pub use frust_widgets::motion;`), mirroring the existing
//! `pub use frust_widgets::icons;` precedent — the only other wholesale
//! module re-export in the facade today. This is deliberate so tasks 16/19's
//! future types ride along under `frust::motion::*` with no further facade
//! edits required.

pub mod animated;
pub mod patterns;
pub mod switcher;

pub use animated::{
    AnimatedOpacity, AnimatedOpacityView, AnimatedOpacityWidget, AnimatedScale, AnimatedScaleView,
    AnimatedScaleWidget, animated_opacity, animated_scale,
};
