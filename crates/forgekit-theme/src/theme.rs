//! [`Theme`]: the aggregate design-token bundle an app wires through to its
//! widget tree (the delivery mechanism itself — context/ctx threading —
//! lands in a later task; this crate only defines the value).

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
    fn m3_baseline_is_internally_consistent() {
        let theme = Theme::m3_baseline();
        assert_eq!(theme.light, ColorScheme::m3_baseline_light());
        assert_eq!(theme.dark, ColorScheme::m3_baseline_dark());
        assert_eq!(theme.brightness, Brightness::Light);
    }
}
