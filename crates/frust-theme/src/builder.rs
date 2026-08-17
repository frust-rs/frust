//! [`ThemeBuilder`]: the `defineTheme`/`copyWith` analog for composing a
//! [`Theme`](crate::theme::Theme) from a baseline plus layered edits.
//!
//! A theme starts from a baseline — [`Theme::neutral`](crate::theme::Theme::neutral),
//! the language-free floor, or any other already-built `Theme` (a design
//! system's own baseline; this is also how a design system assembles that
//! baseline in the first place) — then layers edits through
//! [`Theme::builder`](crate::theme::Theme::builder)
//! in **application order** — whichever call runs last for a given group wins
//! (`ThemeExtensions`' own last-write-wins `insert` for extensions, mirrored
//! here at the group level):
//!
//! 1. **baseline** — the `Theme` passed to [`Theme::builder`](crate::theme::Theme::builder).
//! 2. **whole-group swaps** — [`ThemeBuilder::colors_light`]/[`colors_dark`](ThemeBuilder::colors_dark)/
//!    [`type_scale`](ThemeBuilder::type_scale)/[`shape`](ThemeBuilder::shape)/
//!    [`elevation`](ThemeBuilder::elevation)/[`motion`](ThemeBuilder::motion)/
//!    [`glass`](ThemeBuilder::glass) replace a whole group's value outright.
//! 3. **per-token closure edits** — the `map_*` counterpart of each group
//!    setter above (`map_colors_light`/`map_colors_dark`/`map_type_scale`/
//!    `map_shape`/`map_elevation`/`map_motion`/`map_glass`) hands the
//!    *current* group value to an `FnOnce(T) -> T`, so a caller writes a
//!    Rust struct-update (`..`) edit instead of restating every field.
//! 4. **extensions** — [`ThemeBuilder::extension`] inserts or replaces a
//!    typed extension (see [`crate::extensions`]), same replace-by-`TypeId`
//!    semantics as [`ThemeExtensions::insert`](crate::extensions::ThemeExtensions::insert).
//!
//! There's deliberately no separate "whole-value setter vs. closure editor,
//! same method name" overload: Rust has no method overloading, so each group
//! gets two distinctly-named methods (the whole-value setter and its `map_*`
//! closure counterpart) rather than one method accepting either shape.
//!
//! [`ThemeBuilder::build`] does no validation — tokens are data (v1; see
//! module docs above), so `build()` simply returns the accumulated `Theme`.
//!
//! # Examples
//!
//! Baseline + a whole-group color swap + a per-token shape edit:
//!
//! ```
//! use frust_theme::{ColorScheme, DesignLanguage, ShapeScale, Theme};
//! use peniko::Color;
//!
//! const BRAND: Color = Color::from_rgb8(0xFF, 0x6A, 0x00);
//!
//! let theme = Theme::builder(Theme::neutral())
//!     .colors_dark(ColorScheme {
//!         primary: BRAND,
//!         ..ColorScheme::neutral_dark()
//!     }) // whole-group swap
//!     .map_shape(|s| ShapeScale { medium: 8.0, ..s }) // per-token closure edit
//!     .design_language(DesignLanguage::Custom("acme"))
//!     .build();
//!
//! assert_eq!(theme.dark.primary, BRAND);
//! assert_eq!(theme.shape.medium, 8.0);
//! // Every other shape token is untouched (struct-update `..` above).
//! assert_eq!(theme.shape.large, ShapeScale::m3().large);
//! ```
//!
//! Attaching a typed extension (see [`crate::extensions`]):
//!
//! ```
//! use frust_theme::Theme;
//!
//! #[derive(Debug, Clone, PartialEq)]
//! struct BrandTokens {
//!     logo_glow: bool,
//! }
//!
//! let theme = Theme::builder(Theme::neutral())
//!     .extension(BrandTokens { logo_glow: true })
//!     .build();
//!
//! assert_eq!(theme.extension::<BrandTokens>(), Some(&BrandTokens { logo_glow: true }));
//! ```

use std::any::Any;

use crate::color::{Brightness, ColorScheme};
use crate::elevation::Elevation;
use crate::glass::GlassScale;
use crate::motion::MotionScheme;
use crate::shape::ShapeScale;
use crate::theme::{DesignLanguage, Theme};
use crate::typography::TypeScale;

/// A layered builder over a [`Theme`] baseline — see the module docs for the
/// full precedence order. Every method takes/returns `Self` by value so calls
/// chain (`Theme::builder(base).colors_dark(..).map_shape(..).build()`).
#[derive(Clone, Debug)]
pub struct ThemeBuilder {
    theme: Theme,
}

impl ThemeBuilder {
    /// Start a builder from `base` — typically
    /// [`Theme::neutral`](crate::theme::Theme::neutral) or a design system's
    /// own baseline, but any already-built `Theme` works (e.g. re-deriving
    /// one theme from another).
    pub fn new(base: Theme) -> Self {
        Self { theme: base }
    }

    // -- light color scheme -------------------------------------------------

    /// Whole-group swap: replace the light [`ColorScheme`] outright.
    pub fn colors_light(mut self, scheme: ColorScheme) -> Self {
        self.theme.light = scheme;
        self
    }

    /// Per-token closure edit: hand the current light [`ColorScheme`] to `f`,
    /// keeping whatever fields it doesn't overwrite (Rust struct-update `..`
    /// ergonomics).
    pub fn map_colors_light(mut self, f: impl FnOnce(ColorScheme) -> ColorScheme) -> Self {
        self.theme.light = f(self.theme.light);
        self
    }

    // -- dark color scheme ---------------------------------------------------

    /// Whole-group swap: replace the dark [`ColorScheme`] outright.
    pub fn colors_dark(mut self, scheme: ColorScheme) -> Self {
        self.theme.dark = scheme;
        self
    }

    /// Per-token closure edit: hand the current dark [`ColorScheme`] to `f`.
    pub fn map_colors_dark(mut self, f: impl FnOnce(ColorScheme) -> ColorScheme) -> Self {
        self.theme.dark = f(self.theme.dark);
        self
    }

    // -- type scale -----------------------------------------------------------

    /// Whole-group swap: replace the [`TypeScale`] outright.
    pub fn type_scale(mut self, scale: TypeScale) -> Self {
        self.theme.type_scale = scale;
        self
    }

    /// Per-token closure edit: hand the current [`TypeScale`] to `f`.
    pub fn map_type_scale(mut self, f: impl FnOnce(TypeScale) -> TypeScale) -> Self {
        self.theme.type_scale = f(self.theme.type_scale);
        self
    }

    // -- shape scale -----------------------------------------------------------

    /// Whole-group swap: replace the [`ShapeScale`] outright.
    pub fn shape(mut self, shape: ShapeScale) -> Self {
        self.theme.shape = shape;
        self
    }

    /// Per-token closure edit: hand the current [`ShapeScale`] to `f`.
    pub fn map_shape(mut self, f: impl FnOnce(ShapeScale) -> ShapeScale) -> Self {
        self.theme.shape = f(self.theme.shape);
        self
    }

    // -- elevation ---------------------------------------------------------

    /// Whole-group swap: replace the [`Elevation`] table outright.
    pub fn elevation(mut self, elevation: Elevation) -> Self {
        self.theme.elevation = elevation;
        self
    }

    /// Per-token closure edit: hand the current [`Elevation`] table to `f`.
    pub fn map_elevation(mut self, f: impl FnOnce(Elevation) -> Elevation) -> Self {
        self.theme.elevation = f(self.theme.elevation);
        self
    }

    // -- motion --------------------------------------------------------------

    /// Whole-group swap: replace the [`MotionScheme`] outright.
    pub fn motion(mut self, motion: MotionScheme) -> Self {
        self.theme.motion = motion;
        self
    }

    /// Per-token closure edit: hand the current [`MotionScheme`] to `f`.
    pub fn map_motion(mut self, f: impl FnOnce(MotionScheme) -> MotionScheme) -> Self {
        self.theme.motion = f(self.theme.motion);
        self
    }

    // -- glass ---------------------------------------------------------------

    /// Whole-group swap: replace the [`GlassScale`] outright.
    pub fn glass(mut self, glass: GlassScale) -> Self {
        self.theme.glass = glass;
        self
    }

    /// Per-token closure edit: hand the current [`GlassScale`] to `f`.
    pub fn map_glass(mut self, f: impl FnOnce(GlassScale) -> GlassScale) -> Self {
        self.theme.glass = f(self.theme.glass);
        self
    }

    // -- scalars ---------------------------------------------------------------

    /// Set the active [`Brightness`] (which of `light`/`dark` [`Theme::scheme`](crate::theme::Theme::scheme)
    /// selects).
    pub fn brightness(mut self, brightness: Brightness) -> Self {
        self.theme.brightness = brightness;
        self
    }

    /// Set the [`DesignLanguage`] tag. Purely a tag (see
    /// [`crate::theme::DesignLanguage`]'s docs) — this does not itself swap
    /// any token group; pair it with the whole-group swaps above when
    /// actually changing baselines.
    pub fn design_language(mut self, design_language: DesignLanguage) -> Self {
        self.theme.design_language = design_language;
        self
    }

    // -- extensions ------------------------------------------------------------

    /// Insert or replace a typed extension (see [`crate::extensions`]) —
    /// replace-by-`TypeId`, same semantics as
    /// [`ThemeExtensions::insert`](crate::extensions::ThemeExtensions::insert).
    pub fn extension<T: Any + Send + Sync>(mut self, ext: T) -> Self {
        self.theme.extensions.insert(ext);
        self
    }

    /// Finish building, returning the accumulated [`Theme`]. No validation
    /// pass in v1 — tokens are data (see module docs).
    pub fn build(self) -> Theme {
        self.theme
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StatusPalette;

    const BRAND: peniko::Color = peniko::Color::from_rgb8(0xFF, 0x6A, 0x00);

    #[test]
    fn round_trip_is_field_for_field_identical() {
        // `Theme::builder(x).build() == x`, for the framework baseline and
        // for a design-system-shaped theme built over it (a differently
        // tagged, differently coloured value — so the round-trip isn't
        // proved against one shape only).
        let base = Theme::neutral();
        let rebuilt = Theme::builder(base.clone()).build();
        assert_eq!(rebuilt, base);

        let design_system = Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Cupertino)
            .map_colors_light(|c| ColorScheme {
                primary: BRAND,
                ..c
            })
            .build();
        assert_ne!(design_system, base);
        let rebuilt = Theme::builder(design_system.clone()).build();
        assert_eq!(rebuilt, design_system);
    }

    #[test]
    fn whole_group_swap_replaces_only_that_group() {
        let base = Theme::neutral();
        let swapped_dark = ColorScheme {
            primary: BRAND,
            ..ColorScheme::neutral_dark()
        };
        let theme = Theme::builder(base.clone())
            .colors_dark(swapped_dark)
            .build();

        assert_eq!(theme.dark.primary, BRAND);
        // Nothing else moved.
        assert_eq!(theme.light, base.light);
        assert_eq!(theme.shape, base.shape);
        assert_eq!(theme.motion, base.motion);
        assert_eq!(theme.elevation, base.elevation);
        assert_eq!(theme.type_scale, base.type_scale);
        assert_eq!(theme.glass, base.glass);
        assert_eq!(theme.brightness, base.brightness);
        assert_eq!(theme.design_language, base.design_language);
    }

    #[test]
    fn closure_edit_changes_only_the_named_token() {
        let base = Theme::neutral();
        let theme = Theme::builder(base.clone())
            .map_shape(|s| ShapeScale { medium: 8.0, ..s })
            .build();

        assert_eq!(theme.shape.medium, 8.0);
        assert_eq!(theme.shape.large, base.shape.large);
        assert_eq!(theme.shape.none, base.shape.none);
        // Untouched groups still match the baseline.
        assert_eq!(theme.light, base.light);
        assert_eq!(theme.motion, base.motion);
    }

    #[test]
    fn extension_insert_round_trips() {
        #[derive(Debug, Clone, PartialEq)]
        struct AppTokens {
            brand_name: &'static str,
        }

        let theme = Theme::builder(Theme::neutral())
            .extension(AppTokens { brand_name: "Acme" })
            .build();

        assert_eq!(
            theme.extension::<AppTokens>(),
            Some(&AppTokens { brand_name: "Acme" })
        );
        // The pre-attached StatusPalette extension (from the baseline)
        // survives alongside the newly-inserted one.
        assert_eq!(
            theme.extension::<StatusPalette>(),
            Some(&StatusPalette::neutral())
        );
    }

    #[test]
    fn extension_replace_by_type_id_last_write_wins() {
        #[derive(Debug, Clone, PartialEq)]
        struct Marker(u32);

        let theme = Theme::builder(Theme::neutral())
            .extension(Marker(1))
            .extension(Marker(2))
            .build();

        assert_eq!(theme.extension::<Marker>(), Some(&Marker(2)));
    }

    #[test]
    fn application_order_is_last_write_wins_per_group() {
        // Two whole-group swaps to the same group: the later call wins.
        let first = ShapeScale {
            medium: 8.0,
            ..ShapeScale::m3()
        };
        let second = ShapeScale {
            medium: 16.0,
            ..ShapeScale::m3()
        };
        let theme = Theme::builder(Theme::neutral())
            .shape(first)
            .shape(second)
            .build();
        assert_eq!(theme.shape.medium, 16.0);

        // A closure edit after a whole-group swap sees the swapped value.
        let theme = Theme::builder(Theme::neutral())
            .shape(first)
            .map_shape(|s| ShapeScale { large: 99.0, ..s })
            .build();
        assert_eq!(theme.shape.medium, 8.0);
        assert_eq!(theme.shape.large, 99.0);
    }

    #[test]
    fn brightness_and_design_language_setters_apply() {
        let theme = Theme::builder(Theme::neutral())
            .brightness(Brightness::Dark)
            .design_language(DesignLanguage::Cupertino)
            .build();
        assert_eq!(theme.brightness, Brightness::Dark);
        assert_eq!(theme.design_language, DesignLanguage::Cupertino);
        assert_eq!(theme.scheme(), &theme.dark);
    }
}
