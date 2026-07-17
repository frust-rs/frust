//! [`Theme`]: the aggregate design-token bundle an app wires through to its
//! widget tree. The value is delivered two ways (task 05): to app code through
//! the reactive context (`use_context::<Theme>()` via the facade), and to
//! widgets through the type-erased `PaintCtx`/`LayoutCtx` theme slot — the
//! [`Theme::from_paint_ctx`]/[`Theme::from_layout_ctx`] wrappers below recover
//! it in one line.

use forgekit_core::widget::{LayoutCtx, PaintCtx};
use forgekit_text::TextStyle;

use crate::color::{Brightness, ColorScheme};
use crate::elevation::Elevation;
use crate::motion::MotionScheme;
use crate::shape::ShapeScale;
use crate::typography::TypeScale;

/// Which design language a [`Theme`] was built from — a `Theme` itself stays
/// a single, language-agnostic aggregate struct (spec §17.1); this is just a
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
    pub brightness: Brightness,
    pub design_language: DesignLanguage,
}

impl Theme {
    /// The Material 3 baseline theme: baseline light/dark color schemes,
    /// the M3 type scale (built from `TextStyle::default()`), the M3 shape
    /// scale, the M3 elevation table, and the M3 Expressive motion scheme.
    /// Starts in [`Brightness::Light`].
    pub fn m3_baseline() -> Self {
        Self {
            light: ColorScheme::m3_baseline_light(),
            dark: ColorScheme::m3_baseline_dark(),
            type_scale: TypeScale::m3(&TextStyle::default()),
            shape: ShapeScale::m3(),
            elevation: Elevation::m3(),
            motion: MotionScheme::m3_expressive(),
            brightness: Brightness::Light,
            design_language: DesignLanguage::Material3,
        }
    }

    /// The Cupertino (iOS) baseline theme: baseline light/dark Cupertino
    /// color schemes, the Cupertino type scale (built from
    /// `TextStyle::default()`), the Cupertino shape scale, the Cupertino
    /// elevation table, and the Cupertino motion scheme. Starts in
    /// [`Brightness::Light`]. Shells still hardcode `Theme::m3_baseline()`
    /// as of this task — wiring a shell to pick this baseline instead is a
    /// later task's (04) override seam, not this one's.
    pub fn cupertino_baseline() -> Self {
        Self {
            light: ColorScheme::cupertino_light(),
            dark: ColorScheme::cupertino_dark(),
            type_scale: TypeScale::cupertino(&TextStyle::default()),
            shape: ShapeScale::cupertino(),
            elevation: Elevation::cupertino(),
            motion: MotionScheme::cupertino(),
            brightness: Brightness::Light,
            design_language: DesignLanguage::Cupertino,
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
    /// Framework footgun this closes (6e Finding 6, bug 2): `set_app_theme`
    /// stores its argument as the override-wins theme (spec's override-wins
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
    /// [`PaintCtx::theme_as::<Theme>()`](forgekit_core::widget::PaintCtx::theme_as)
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
        use forgekit_core::widget::PaintCtx;
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
        use forgekit_core::widget::LayoutCtx;

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
        // Regression for 6e Finding 6, bug 2: forcing a design language via a
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
}
