//! [`Theme`]: the aggregate design-token bundle an app wires through to its
//! widget tree. The value is delivered two ways: to app code through
//! the reactive context (`use_context::<Theme>()` via the facade), and to
//! widgets through the type-erased `PaintCtx`/`LayoutCtx` theme slot — the
//! [`Theme::from_paint_ctx`]/[`Theme::from_layout_ctx`] wrappers below recover
//! it in one line.

use std::any::Any;

use frust_core::widget::{LayoutCtx, PaintCtx};
use frust_text::TextStyle;

use crate::color::{Brightness, ColorScheme};
use crate::elevation::Elevation;
use crate::extensions::ThemeExtensions;
use crate::glass::GlassScale;
use crate::motion::MotionScheme;
use crate::shape::ShapeScale;
use crate::status::StatusPalette;
use crate::typography::TypeScale;

/// Which design language assembled a [`Theme`] — a `Theme` itself stays a
/// single, language-agnostic aggregate struct; this is just a tag app/shell
/// code can branch on (e.g. to pick per-platform interaction affordances),
/// not a second `Theme` type. No constructor in this crate produces anything
/// but the derived default: a design system sets its own tag through
/// [`ThemeBuilder::design_language`](crate::builder::ThemeBuilder::design_language)
/// while assembling its baseline, from its own crate.
///
/// The tag is identity, not behavior: a design system styles itself entirely
/// via `Theme`'s tokens plus `Theme::extensions` (see
/// [`ThemeExtensions`](crate::extensions::ThemeExtensions)) — this enum just
/// lets a host/widget recognize which system is active, or deliberately
/// ignore an unrecognized one. `#[non_exhaustive]` and [`Custom`](Self::Custom)
/// together mean a new built-in variant, or a third-party system tagging
/// itself, is never a breaking change for downstream code: the built-in
/// `==`-based branch site (`frust-widgets`' slider) already treats anything
/// that isn't `Cupertino` as the neutral path by construction, so an
/// unrecognized `Custom` id falls through safely. Two `Custom` tags compare
/// equal by string content, not by pointer/interning identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum DesignLanguage {
    /// Material 3 (Android/cross-platform baseline). Default — the tag
    /// `Theme::neutral` carries for want of a neutral variant (see that
    /// constructor's caveat), and the one `frust-material` sets explicitly.
    #[default]
    Material3,
    /// Cupertino (iOS) — the tag `frust-cupertino` sets.
    Cupertino,
    /// Glyph — the tag `frust-glyph` sets.
    Glyph,
    /// A third-party design system's identity tag — the id is the system's
    /// stable name (e.g. `"yaru"`). Carried so hosts/widgets that branch on
    /// language can recognize (or deliberately ignore) an external system;
    /// the built-in `==` branch site (the slider) treats any `Custom` as the
    /// neutral path by construction. Compared by string content —
    /// `Custom("yaru") == Custom("yaru")` even across two distinct
    /// `&'static str` allocations with the same bytes.
    Custom(&'static str),
}

/// A full design-token bundle: paired light/dark color schemes, the type
/// scale, shape scale, elevation table, and motion scheme, plus which
/// brightness is currently active.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub light: ColorScheme,
    pub dark: ColorScheme,
    pub type_scale: TypeScale,
    pub shape: ShapeScale,
    pub elevation: Elevation,
    pub motion: MotionScheme,
    /// The glass material scale. [`GlassScale::opaque_material`] is the only
    /// recipe this crate constructs (and what [`Theme::neutral`] carries); a
    /// design system with translucent chrome supplies its own through
    /// [`ThemeBuilder::glass`](crate::builder::ThemeBuilder::glass). Consumed
    /// by chrome widgets that branch on
    /// [`GlassMaterial::is_opaque`](crate::glass::GlassMaterial::is_opaque).
    pub glass: GlassScale,
    pub brightness: Brightness,
    pub design_language: DesignLanguage,
    /// The no-lock-in typed extension slot (a Flutter
    /// `ThemeExtension` analog) — see `crate::extensions` and
    /// [`Theme::extension`]. `Arc`-backed internally, so cloning a `Theme`
    /// (required at both delivery paths — the process-global override slot
    /// and the reactive `provide_context` copy) stays cheap regardless of
    /// how many extensions are attached.
    pub extensions: ThemeExtensions,
}

impl Theme {
    /// The neutral, design-language-free baseline theme — the **only**
    /// `Theme` this crate constructs, and the floor every shell falls back to
    /// when no design system seeded one via `set_default_theme` (see
    /// `docs/ARCHITECTURE.md`'s Theme delivery). Material, Cupertino, and
    /// Glyph each assemble their own baseline in their own plugin crate.
    ///
    /// Composes: a plain grayscale surface/on-surface ramp plus one
    /// restrained slate-blue accent
    /// ([`ColorScheme::neutral_light`]/[`ColorScheme::neutral_dark`]); a
    /// numeric type scale resolved against a generic system-font stack with
    /// **no bundled font bytes referenced** ([`TypeScale::neutral`]); the
    /// [`ShapeScale::m3`]/[`Elevation::m3`] value tables — reused rather than
    /// re-authored, since neither is actually M3-branded in *value*
    /// (`Elevation::m3`'s own module docs call its shadow math "TUNABLE,
    /// not an M3-published spec", and [`GlassScale::opaque_material`]
    /// already reuses `Elevation::m3` the same way); a no-overshoot
    /// [`MotionScheme::neutral`]; and [`GlassScale::opaque_material`]
    /// (already neutral). Attaches [`StatusPalette::neutral`], since
    /// success/warning/info are a functional signal, not a design-language
    /// "look" — the same reasoning `neutral_light`/`neutral_dark` use to keep
    /// `error` real red instead of grayscaling it too.
    ///
    /// Starts in [`Brightness::Light`].
    ///
    /// **Caveat — `design_language` is [`DesignLanguage::Material3`] here**,
    /// the derived default, despite this baseline carrying no Material
    /// identity: the enum has no neutral variant and is not reshaped by this
    /// constructor. Branch on the tokens you actually need, not on this field,
    /// when handed a `neutral()` theme.
    ///
    /// Not `const`: `ThemeExtensions`' `HashMap` construction isn't
    /// const-evaluable.
    pub fn neutral() -> Self {
        let mut extensions = ThemeExtensions::new();
        extensions.insert(StatusPalette::neutral());
        Self {
            light: ColorScheme::neutral_light(),
            dark: ColorScheme::neutral_dark(),
            type_scale: TypeScale::neutral(&TextStyle::default()),
            shape: ShapeScale::m3(),
            elevation: Elevation::m3(),
            motion: MotionScheme::neutral(),
            glass: GlassScale::opaque_material(),
            brightness: Brightness::Light,
            design_language: DesignLanguage::default(),
            extensions,
        }
    }

    /// The active [`ColorScheme`] — `light` or `dark`, selected by
    /// `self.brightness`.
    pub fn scheme(&self) -> &ColorScheme {
        match self.brightness {
            Brightness::Light => &self.light,
            Brightness::Dark => &self.dark,
        }
    }

    /// Force this theme's [`Brightness`] while keeping every other token —
    /// a builder over the `brightness` field, meant to be chained onto a
    /// baseline constructor (which hardcodes `Brightness::Light`) before
    /// handing the result to `set_app_theme`.
    ///
    /// Framework footgun this closes: `set_app_theme`
    /// stores its argument as the override-wins theme (the override-wins
    /// rule — an app-set theme always beats further OS appearance reports,
    /// intentionally). A caller that forces a design language via
    /// `set_app_theme(some_baseline())` therefore also silently pins
    /// brightness to `Light` forever, discarding whatever OS night-mode state
    /// was live a moment before. `some_baseline().with_brightness(live)`
    /// forces the design language without discarding brightness.
    pub fn with_brightness(mut self, brightness: Brightness) -> Self {
        self.brightness = brightness;
        self
    }

    /// Recover the active theme from a widget's [`PaintCtx`], or `None` if none
    /// was threaded into the paint pass (a supported state — a pre-theme app or
    /// a bare-core test).
    ///
    /// A one-line convenience wrapper over
    /// [`PaintCtx::theme_as::<Theme>()`](frust_core::widget::PaintCtx::theme_as)
    /// so a themed widget writes `Theme::from_paint_ctx(ctx)` in its `paint`.
    pub fn from_paint_ctx<'a>(ctx: &'a PaintCtx<'_>) -> Option<&'a Theme> {
        ctx.theme_as::<Theme>()
    }

    /// Recover the active theme from a widget's [`LayoutCtx`], or `None` if none
    /// was threaded into the layout pass. The layout-pass mirror of
    /// [`Theme::from_paint_ctx`].
    pub fn from_layout_ctx<'a>(ctx: &'a LayoutCtx<'_>) -> Option<&'a Theme> {
        ctx.theme_as::<Theme>()
    }

    /// Recover a typed extension previously attached via
    /// `self.extensions.insert::<T>(..)`, or `None` if nothing of that type
    /// was ever attached — see `crate::extensions`' module docs for the
    /// no-lock-in rationale. A one-line convenience wrapper over
    /// [`ThemeExtensions::get`].
    pub fn extension<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.extensions.get::<T>()
    }

    /// Start a [`crate::builder::ThemeBuilder`] over `self` as the baseline —
    /// the `defineTheme`/`copyWith` analog. See
    /// `crate::builder`'s module docs for the full layered-precedence
    /// contract (baseline → whole-group swaps → per-token closure edits →
    /// extensions).
    pub fn builder(base: Theme) -> crate::builder::ThemeBuilder {
        crate::builder::ThemeBuilder::new(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_selects_by_brightness() {
        let mut theme = Theme::neutral();
        assert_eq!(theme.scheme(), &theme.light);

        theme.brightness = Brightness::Dark;
        assert_eq!(theme.scheme(), &theme.dark);
    }

    #[test]
    fn from_paint_ctx_recovers_a_threaded_theme() {
        use frust_core::widget::PaintCtx;
        use peniko::kurbo::{Point, Size};

        let theme = Theme::neutral();
        let ctx = PaintCtx::new(Point::ZERO, Size::new(1.0, 1.0)).with_theme(&theme);
        assert_eq!(Theme::from_paint_ctx(&ctx), Some(&theme));

        // No theme threaded in → `None`, never a panic.
        let bare = PaintCtx::new(Point::ZERO, Size::new(1.0, 1.0));
        assert!(Theme::from_paint_ctx(&bare).is_none());
    }

    #[test]
    fn from_layout_ctx_recovers_a_threaded_theme() {
        use frust_core::widget::LayoutCtx;

        let theme = Theme::neutral();
        let ctx = LayoutCtx::new().with_theme(&theme);
        assert_eq!(Theme::from_layout_ctx(&ctx), Some(&theme));

        let bare = LayoutCtx::new();
        assert!(Theme::from_layout_ctx(&bare).is_none());
    }

    #[test]
    fn design_language_defaults_to_material3() {
        assert_eq!(DesignLanguage::default(), DesignLanguage::Material3);
    }

    #[test]
    fn with_brightness_forces_design_language_without_discarding_brightness() {
        // Regression: forcing a design language by handing `set_app_theme` a
        // design system's baseline used to silently reset brightness to
        // `Light` even when the caller's live brightness was `Dark`. The two
        // themes below stand in for a design system's own baseline — built
        // inline here since none is constructible from this crate any more,
        // and deliberately carrying DIFFERENT tags so the assertion that the
        // tag survives is not vacuous.
        let material = Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Material3)
            .build();
        let cupertino = Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Cupertino)
            .build();
        assert_ne!(material.design_language, cupertino.design_language);

        let theme = material.clone().with_brightness(Brightness::Dark);
        assert_eq!(theme.brightness, Brightness::Dark);
        assert_eq!(theme.design_language, DesignLanguage::Material3);
        assert_eq!(theme.scheme(), &theme.dark);

        let theme = cupertino.with_brightness(Brightness::Dark);
        assert_eq!(theme.brightness, Brightness::Dark);
        assert_eq!(theme.design_language, DesignLanguage::Cupertino);
        assert_eq!(theme.scheme(), &theme.dark);

        // Light stays light — the builder isn't a one-way flip.
        let theme = material.with_brightness(Brightness::Light);
        assert_eq!(theme.brightness, Brightness::Light);
    }

    #[test]
    fn custom_extension_type_round_trips() {
        // A custom user type round-trips
        // insert -> get, alongside the pre-attached `StatusPalette`.
        #[derive(Debug, PartialEq)]
        struct AppTokens {
            brand_name: &'static str,
        }

        let mut theme = Theme::neutral();
        assert!(theme.extension::<AppTokens>().is_none());

        theme.extensions.insert(AppTokens { brand_name: "Acme" });
        assert_eq!(
            theme.extension::<AppTokens>(),
            Some(&AppTokens { brand_name: "Acme" })
        );

        // The pre-attached extension is unaffected by inserting another type.
        use crate::status::StatusPalette;
        assert_eq!(
            theme.extension::<StatusPalette>(),
            Some(&StatusPalette::neutral())
        );
    }

    #[test]
    fn theme_extensions_field_survives_clone() {
        #[derive(Debug, PartialEq)]
        struct Marker;

        let mut theme = Theme::neutral();
        theme.extensions.insert(Marker);

        let cloned = theme.clone();
        assert_eq!(cloned.extension::<Marker>(), Some(&Marker));
    }

    // ---- Neutral baseline --------------------------------------------

    #[test]
    fn neutral_populates_every_role_in_both_brightnesses() {
        // A fully-populated `Theme`, no placeholder — every field below
        // constructs and round-trips through the aggregate untouched.
        let theme = Theme::neutral();
        assert_eq!(theme.light, ColorScheme::neutral_light());
        assert_eq!(theme.dark, ColorScheme::neutral_dark());
        assert_eq!(theme.shape, ShapeScale::m3());
        assert_eq!(theme.elevation, Elevation::m3());
        assert_eq!(theme.motion, MotionScheme::neutral());
        assert_eq!(theme.glass, GlassScale::opaque_material());
        assert_eq!(theme.brightness, Brightness::Light);
    }

    #[test]
    fn neutral_with_brightness_selects_both_schemes() {
        // Both brightnesses of `neutral()` are legible and distinct — the
        // dark scheme is reachable the same way every other baseline's is.
        let light = Theme::neutral();
        assert_eq!(light.scheme(), &light.light);

        let dark = Theme::neutral().with_brightness(Brightness::Dark);
        assert_eq!(dark.scheme(), &dark.dark);
        assert_ne!(dark.scheme().surface, light.scheme().surface);
    }

    #[test]
    fn neutral_attaches_the_neutral_status_palette_extension() {
        use crate::status::StatusPalette;
        // Success/warning/info are a functional signal, not a "look" — the
        // same reasoning `error` stays real red in `ColorScheme::neutral_*`
        // (see that constructor's doc comment).
        assert_eq!(
            Theme::neutral().extension::<StatusPalette>(),
            Some(&StatusPalette::neutral())
        );
    }

    #[test]
    fn neutral_design_language_stays_the_default() {
        // The enum has no neutral variant, so this baseline carries the
        // derived default (see the constructor's own caveat) rather than
        // claiming a design language it doesn't have.
        assert_eq!(Theme::neutral().design_language, DesignLanguage::default());
    }

    #[test]
    fn neutral_type_scale_references_no_bundled_font() {
        // Exercised through the full `Theme` (see `crate::typography`'s own
        // tests for the focused check).
        let theme = Theme::neutral();
        assert!(matches!(
            theme.type_scale.body_large.family,
            frust_text::FontFamily::NamedWithGeneric(_)
        ));
    }

    #[test]
    fn neutral_survives_clone_and_extends_independently() {
        let theme = Theme::neutral();
        let cloned = theme.clone();
        assert_eq!(cloned, theme);
    }

    // ---- DesignLanguage::Custom ---------------------------------------

    #[test]
    fn custom_design_language_compares_by_content() {
        assert_eq!(DesignLanguage::Custom("x"), DesignLanguage::Custom("x"));
        assert_ne!(DesignLanguage::Custom("x"), DesignLanguage::Custom("y"));
        assert_ne!(DesignLanguage::Custom("x"), DesignLanguage::Material3);
    }

    #[test]
    fn custom_design_language_round_trips_through_the_builder() {
        let theme = Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Custom("sample"))
            .build();
        assert_eq!(theme.design_language, DesignLanguage::Custom("sample"));
    }
}
