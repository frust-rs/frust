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
    }
}
