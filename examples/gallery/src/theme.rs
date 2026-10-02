//! [`theme`]: resolve a [`Design`]/[`Variant`] pairing into a concrete
//! [`frust_theme::Theme`] — the one place this crate maps a case's design tag
//! onto the design system's own public baseline constructor.

use frust_theme::{Brightness, Theme};

use crate::case::{Design, Variant};

/// The theme a [`Design`]/[`Variant`] pairing resolves to: each design
/// system's own baseline theme constructor
/// (`frust_material::baseline`/`frust_cupertino::baseline`/
/// `frust_glyph::baseline`/`frust_shadcn::theme`/`frust_beui::theme`),
/// `Base` falling back to [`Theme::neutral`] — with
/// [`Theme::with_brightness`] forcing the requested [`Variant`].
///
/// Every baseline constructor above returns a `Theme` carrying BOTH light and
/// dark [`frust_theme::ColorScheme`]s regardless of which brightness it
/// starts in (Glyph starts dark-first, everything else starts light-first) —
/// `with_brightness` only selects which of the two is active
/// ([`Theme::scheme`]), so this is a total function: every `(Design,
/// Variant)` pairing produces a valid theme (see `tests/registry.rs`).
pub fn theme(design: Design, variant: Variant) -> Theme {
    let brightness = match variant {
        Variant::Light => Brightness::Light,
        Variant::Dark => Brightness::Dark,
    };
    let base = match design {
        Design::Base => Theme::neutral(),
        Design::Material => frust_material::baseline(),
        Design::Cupertino => frust_cupertino::baseline(),
        Design::Glyph => frust_glyph::baseline(),
        Design::Shadcn => frust_shadcn::theme(),
        Design::Beui => frust_beui::theme(),
    };
    base.with_brightness(brightness)
}
