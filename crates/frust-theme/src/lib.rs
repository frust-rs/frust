//! Design-token crate: Material 3 baseline color/type/shape/elevation/motion
//! values, their Cupertino (iOS) counterparts, plus the [`Theme`] aggregate
//! that bundles either into one design-language-agnostic struct (a
//! [`DesignLanguage`] tag says which baseline it was built from — see
//! `color`'s "Cupertino (iOS) mapping" module docs for the full role
//! mapping).
//!
//! This crate is deliberately **pure data + constructors** — no
//! `frust-core`, no `frust-scene`, no `reactive_graph`. It depends on
//! `peniko` (for [`peniko::Color`], reused by [`color::ColorScheme`]) and
//! `frust-text` (for [`frust_text::TextStyle`], reused by
//! [`typography::TypeScale`]) only — see `docs/ARCHITECTURE.md`'s Layer
//! Dependencies. The `resolve`/context-threading helpers that make a `Theme`
//! reachable from widget code land in a later task (05); this crate only
//! defines the values.
//!
//! Every token table below carries its own source URL and retrieval date in
//! its module's doc comments.

pub mod builder;
pub mod color;
pub mod elevation;
pub mod extensions;
pub mod glass;
#[cfg(feature = "glyph")]
pub mod glyph;
pub mod motion;
pub mod shape;
pub mod status;
pub mod theme;
pub mod typography;

pub use builder::ThemeBuilder;
pub use color::{Brightness, ColorScheme};
pub use elevation::{Elevation, ElevationLevel, ShadowSpec, SurfaceRole};
pub use extensions::ThemeExtensions;
pub use glass::{GlassFill, GlassMaterial, GlassScale};
#[cfg(feature = "glyph")]
pub use glyph::GlyphInk;
pub use motion::{CosmeticLoopRate, EasingSet, MotionDurations, MotionScheme, MotionSpring};
pub use shape::ShapeScale;
pub use status::{StatusColors, StatusPalette};
pub use theme::{DesignLanguage, Theme};
pub use typography::TypeScale;
