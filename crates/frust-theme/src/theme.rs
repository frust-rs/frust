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

/// Which design language a [`Theme`] was built from — a `Theme` itself stays
/// a single, language-agnostic aggregate struct; this is just a
/// tag app/shell code can branch on (e.g. to pick per-platform interaction
/// affordances), not a second `Theme` type. See `crate::color`'s "Cupertino
/// (iOS) mapping" module docs for how [`ColorScheme::cupertino_light`]/
/// [`ColorScheme::cupertino_dark`] fill the same 46 roles [`Theme::cupertino_baseline`]
/// uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DesignLanguage {
    /// Material 3 (Android/cross-platform baseline). Default.
    #[default]
    Material3,
    /// Cupertino (iOS).
    Cupertino,
    /// Glyph — this crate's own design language. Not the derived-default
    /// enum variant; [`Theme::glyph_baseline`] is what shells construct
    /// explicitly to use it.
    Glyph,
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
    /// The glass material scale: [`GlassScale::opaque_material`]
    /// on the Material baseline, [`GlassScale::ios27`] on Cupertino.
    /// Consumed by the Cupertino chrome widgets that paint from it.
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
    /// The Material 3 baseline theme: baseline light/dark color schemes,
    /// the M3 type scale (built from `TextStyle::default()`), the M3 shape
    /// scale, the M3 elevation table, and the M3 Expressive motion scheme.
    /// Starts in [`Brightness::Light`]. Attaches [`StatusPalette::m3`] as a
    /// pre-populated extension (see [`Theme::extension`]) so
    /// `extension::<StatusPalette>()` is always `Some` on this baseline.
    ///
    /// Not `const` (unlike the `ColorScheme`/`ShapeScale`/… constructors it
    /// composes): `ThemeExtensions`' internal `HashMap` construction isn't
    /// const-evaluable, and this constructor was already a plain `fn` before
    /// this field existed (no caller relies on const-ness).
    pub fn m3_baseline() -> Self {
        let mut extensions = ThemeExtensions::new();
        extensions.insert(StatusPalette::m3());
        Self {
            light: ColorScheme::m3_baseline_light(),
            dark: ColorScheme::m3_baseline_dark(),
            type_scale: TypeScale::m3(&TextStyle::default()),
            shape: ShapeScale::m3(),
            elevation: Elevation::m3(),
            motion: MotionScheme::m3_expressive(),
            glass: GlassScale::opaque_material(),
            brightness: Brightness::Light,
            design_language: DesignLanguage::Material3,
            extensions,
        }
    }

    /// The Cupertino (iOS) baseline theme: baseline light/dark Cupertino
    /// color schemes, the Cupertino type scale (built from
    /// `TextStyle::default()`), the Cupertino shape scale, the Cupertino
    /// elevation table, and the Cupertino motion scheme. Starts in
    /// [`Brightness::Light`]. Shells still hardcode `Theme::m3_baseline()`
    /// as of this task — wiring a shell to pick this baseline instead is a
    /// later task's (04) override seam, not this one's. Attaches
    /// [`StatusPalette::m3`] as a pre-populated extension (Cupertino has no
    /// published success/warning/info equivalent either, so it shares the
    /// M3 default rather than going unset — see [`Theme::extension`]).
    pub fn cupertino_baseline() -> Self {
        let mut extensions = ThemeExtensions::new();
        extensions.insert(StatusPalette::m3());
        Self {
            light: ColorScheme::cupertino_light(),
            dark: ColorScheme::cupertino_dark(),
            type_scale: TypeScale::cupertino(&TextStyle::default()),
            shape: ShapeScale::cupertino(),
            elevation: Elevation::cupertino(),
            motion: MotionScheme::cupertino(),
            glass: GlassScale::ios27(),
            brightness: Brightness::Light,
            design_language: DesignLanguage::Cupertino,
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

    /// Force this theme's [`DesignLanguage`]/[`Brightness`] combination while
    /// preserving a live brightness — a builder over the `brightness` field,
    /// meant to be chained onto [`Theme::m3_baseline`]/[`Theme::cupertino_baseline`]
    /// (both of which hardcode `Brightness::Light`) before handing the result to
    /// `set_app_theme`.
    ///
    /// Framework footgun this closes: `set_app_theme`
    /// stores its argument as the override-wins theme (the override-wins
    /// rule — an app-set theme always beats further OS appearance reports,
    /// intentionally). A caller that forces a design language via
    /// `set_app_theme(Theme::m3_baseline())` therefore also silently pins
    /// brightness to `Light` forever, discarding whatever OS night-mode state
    /// was live a moment before. `Theme::m3_baseline().with_brightness(live)`
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
        let mut theme = Theme::m3_baseline();
        assert_eq!(theme.scheme(), &theme.light);

        theme.brightness = Brightness::Dark;
        assert_eq!(theme.scheme(), &theme.dark);
    }

    #[test]
    fn from_paint_ctx_recovers_a_threaded_theme() {
        use frust_core::widget::PaintCtx;
        use peniko::kurbo::{Point, Size};

        let theme = Theme::m3_baseline();
        let ctx = PaintCtx::new(Point::ZERO, Size::new(1.0, 1.0)).with_theme(&theme);
        assert_eq!(Theme::from_paint_ctx(&ctx), Some(&theme));

        // No theme threaded in → `None`, never a panic.
        let bare = PaintCtx::new(Point::ZERO, Size::new(1.0, 1.0));
        assert!(Theme::from_paint_ctx(&bare).is_none());
    }

    #[test]
    fn from_layout_ctx_recovers_a_threaded_theme() {
        use frust_core::widget::LayoutCtx;

        let theme = Theme::m3_baseline();
        let ctx = LayoutCtx::new().with_theme(&theme);
        assert_eq!(Theme::from_layout_ctx(&ctx), Some(&theme));

        let bare = LayoutCtx::new();
        assert!(Theme::from_layout_ctx(&bare).is_none());
    }

    #[test]
    fn m3_baseline_is_internally_consistent() {
        let theme = Theme::m3_baseline();
        assert_eq!(theme.light, ColorScheme::m3_baseline_light());
        assert_eq!(theme.dark, ColorScheme::m3_baseline_dark());
        assert_eq!(theme.brightness, Brightness::Light);
        assert_eq!(theme.design_language, DesignLanguage::Material3);
    }

    #[test]
    fn cupertino_baseline_is_internally_consistent() {
        let theme = Theme::cupertino_baseline();
        assert_eq!(theme.light, ColorScheme::cupertino_light());
        assert_eq!(theme.dark, ColorScheme::cupertino_dark());
        assert_eq!(theme.brightness, Brightness::Light);
        assert_eq!(theme.design_language, DesignLanguage::Cupertino);
    }

    #[test]
    fn baselines_carry_the_matching_glass_scale() {
        use crate::glass::GlassScale;
        // Material baseline → opaque glass; Cupertino baseline → iOS-27 glass.
        assert_eq!(Theme::m3_baseline().glass, GlassScale::opaque_material());
        assert_eq!(Theme::cupertino_baseline().glass, GlassScale::ios27());
        assert!(Theme::m3_baseline().glass.chrome.is_opaque());
        assert!(!Theme::cupertino_baseline().glass.chrome.is_opaque());
    }

    #[test]
    fn design_language_defaults_to_material3() {
        assert_eq!(DesignLanguage::default(), DesignLanguage::Material3);
    }

    #[test]
    fn cupertino_scheme_selects_by_brightness() {
        let mut theme = Theme::cupertino_baseline();
        assert_eq!(theme.scheme(), &theme.light);

        theme.brightness = Brightness::Dark;
        assert_eq!(theme.scheme(), &theme.dark);
    }

    #[test]
    fn with_brightness_forces_design_language_without_discarding_brightness() {
        // Regression: forcing a design language via a
        // baseline constructor used to silently reset brightness to `Light`
        // even when the caller's live brightness was `Dark`.
        let theme = Theme::m3_baseline().with_brightness(Brightness::Dark);
        assert_eq!(theme.brightness, Brightness::Dark);
        assert_eq!(theme.design_language, DesignLanguage::Material3);
        assert_eq!(theme.scheme(), &theme.dark);

        let theme = Theme::cupertino_baseline().with_brightness(Brightness::Dark);
        assert_eq!(theme.brightness, Brightness::Dark);
        assert_eq!(theme.design_language, DesignLanguage::Cupertino);
        assert_eq!(theme.scheme(), &theme.dark);

        // Light stays light — the builder isn't a one-way flip.
        let theme = Theme::m3_baseline().with_brightness(Brightness::Light);
        assert_eq!(theme.brightness, Brightness::Light);
    }

    #[test]
    fn both_baselines_carry_a_status_palette_extension() {
        // `extension::<StatusPalette>()` is `Some`
        // for both built-in baselines.
        use crate::status::StatusPalette;

        assert_eq!(
            Theme::m3_baseline().extension::<StatusPalette>(),
            Some(&StatusPalette::m3())
        );
        assert_eq!(
            Theme::cupertino_baseline().extension::<StatusPalette>(),
            Some(&StatusPalette::m3())
        );
    }

    #[test]
    fn custom_extension_type_round_trips() {
        // A custom user type round-trips
        // insert -> get, alongside the pre-attached `StatusPalette`.
        #[derive(Debug, PartialEq)]
        struct AppTokens {
            brand_name: &'static str,
        }

        let mut theme = Theme::m3_baseline();
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
            Some(&StatusPalette::m3())
        );
    }

    #[test]
    fn theme_extensions_field_survives_clone() {
        #[derive(Debug, PartialEq)]
        struct Marker;

        let mut theme = Theme::m3_baseline();
        theme.extensions.insert(Marker);

        let cloned = theme.clone();
        assert_eq!(cloned.extension::<Marker>(), Some(&Marker));
    }
}
