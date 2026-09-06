//! Design-token crate: the [`Theme`] aggregate and its token-group tables
//! (color, typography, shape, elevation, motion, glass, status), plus the
//! [`ThemeBuilder`] and [`ThemeExtensions`] seams a design system composes
//! its own token set through.
//!
//! **No design language lives here.** The one baseline this crate constructs
//! is [`Theme::neutral`](theme::Theme::neutral) — the language-free floor
//! every shell falls back to when nothing was installed. Material, Cupertino,
//! and Glyph each ship as their own plugin crate (`frust-material`,
//! `frust-cupertino`, `frust-glyph`), building their tables over the same
//! public constructors and attaching whatever they need through
//! [`ThemeExtensions`]. A [`DesignLanguage`] tag records *which* system
//! assembled a `Theme`; it never selects behaviour here.
//!
//! This crate is deliberately **pure data + constructors** — no
//! `frust-scene`, no `reactive_graph`. It depends on `peniko` (for
//! [`peniko::Color`], reused by [`color::ColorScheme`]), `frust-text` (for
//! [`frust_text::TextStyle`], reused by [`typography::TypeScale`]), and
//! `frust-core` (for the `PaintCtx`/`LayoutCtx` recovery helpers alone) — see
//! `docs/ARCHITECTURE.md`'s Layer Dependencies.
//!
//! Every token table below carries its own source URL and retrieval date in
//! its module's doc comments.

pub mod builder;
pub mod color;
pub mod elevation;
pub mod extensions;
pub mod glass;
pub mod motion;
pub mod shape;
pub mod status;
pub mod theme;
pub mod typefaces;
pub mod typography;

pub use builder::ThemeBuilder;
pub use color::{Brightness, ColorScheme};
pub use elevation::{Elevation, ElevationLevel, ShadowSpec, SurfaceRole};
pub use extensions::ThemeExtensions;
pub use glass::{GlassFill, GlassMaterial, GlassScale};
pub use motion::{CosmeticLoopRate, EasingSet, MotionDurations, MotionScheme, MotionSpring};
pub use shape::ShapeScale;
pub use status::{StatusColors, StatusPalette};
pub use theme::{DesignLanguage, Theme};
pub use typefaces::{FontFace, NativeTypefaces};
pub use typography::TypeScale;
