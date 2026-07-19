//! [`TextStyle`]: the styling knobs applied uniformly to a laid-out string —
//! family, weight, style (italic/oblique), size, color, letter-spacing, and
//! line-height (the M3 type-scale surface).
//!
//! Every type here is Frust-owned, not a parley re-export (scene-layer
//! purity, see `docs/ARCHITECTURE.md`): [`FontFamily`]/[`FontWeight`]/
//! [`FontStyle`]/[`LineHeight`] mirror parley 0.11's own semantics (see
//! `parley::style`) so the conversion in [`crate::context`]/[`crate::editor`]
//! is a straight match, but no parley type appears in this module's public
//! API. Rich per-range styling and bundled/custom font registration are
//! deferred.

use peniko::Color;

/// Font family selection.
///
/// The default, [`FontFamily::SystemUi`], resolves to the platform UI font
/// (e.g. San Francisco on macOS) via fontique's system collection with no
/// registration required. [`FontFamily::Named`] is an ordered fallback stack
/// of named families, tried in order; a name that doesn't resolve on the
/// current platform falls back per parley's own fallback behavior — no error
/// surface is exposed here.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum FontFamily {
    /// The platform system UI font. Default.
    #[default]
    SystemUi,
    /// An ordered fallback stack of named font families.
    Named(Vec<String>),
}

impl FontFamily {
    /// A single named font family.
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named(vec![name.into()])
    }

    /// An ordered fallback stack of named font families, tried in order.
    pub fn stack(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Named(names.into_iter().map(Into::into).collect())
    }
}

/// Visual weight of a font, on the standard 1-1000 CSS `font-weight` scale.
///
/// Mirrors parley/fontique's `FontWeight` (see `parley::FontWeight`) without
/// leaking the type itself past this crate's boundary.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FontWeight(f32);

impl FontWeight {
    /// Weight value of 100.
    pub const THIN: Self = Self(100.0);
    /// Weight value of 200.
    pub const EXTRA_LIGHT: Self = Self(200.0);
    /// Weight value of 300.
    pub const LIGHT: Self = Self(300.0);
    /// Weight value of 400. Default.
    pub const REGULAR: Self = Self(400.0);
    /// Weight value of 500.
    pub const MEDIUM: Self = Self(500.0);
    /// Weight value of 600.
    pub const SEMI_BOLD: Self = Self(600.0);
    /// Weight value of 700.
    pub const BOLD: Self = Self(700.0);
    /// Weight value of 800.
    pub const EXTRA_BOLD: Self = Self(800.0);
    /// Weight value of 900.
    pub const BLACK: Self = Self(900.0);

    /// A custom weight value (1-1000 scale by convention; not clamped).
    pub const fn new(value: f32) -> Self {
        Self(value)
    }

    /// The underlying numeric weight value.
    pub const fn value(self) -> f32 {
        self.0
    }
}

impl Default for FontWeight {
    fn default() -> Self {
        Self::REGULAR
    }
}

/// Visual slant of a font.
///
/// Mirrors parley/fontique's `FontStyle` (see `parley::FontStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FontStyle {
    /// An upright or "roman" style. Default.
    #[default]
    Normal,
    /// A slanted style, generally with a different structure from the
    /// normal style.
    Italic,
    /// A slanted style derived from the normal style, with an optional angle
    /// in degrees (`None` uses the engine-specific default angle).
    Oblique(Option<f32>),
}

/// Line height: how much vertical space each line of text occupies.
///
/// Mirrors parley's `LineHeight` semantics (see `parley::LineHeight`)
/// without leaking the parley type. M3's type scale specifies line heights
/// in absolute logical pixels ([`LineHeight::Absolute`]);
/// [`LineHeight::FontSizeRelative`] is the documented M3-friendly choice when
/// a consistent, font-independent ratio is preferred instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    /// A multiple of the font's own metrics-derived line height (ascender +
    /// descender + line gap/leading). `1.0` is the font's natural line
    /// height.
    MetricsRelative(f32),
    /// A multiple of the font size — CSS's unitless `line-height`. Useful
    /// for consistent line heights across platforms/fonts when using
    /// system-defined generic families.
    FontSizeRelative(f32),
    /// An absolute line height in logical pixels.
    Absolute(f32),
}

impl Default for LineHeight {
    /// Matches parley's own default: the font's natural metrics-relative
    /// line height.
    fn default() -> Self {
        Self::MetricsRelative(1.0)
    }
}

/// Styling applied uniformly to a laid-out string.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Font family (or fallback stack).
    pub family: FontFamily,
    /// Font weight.
    pub weight: FontWeight,
    /// Font style (upright/italic/oblique).
    pub style: FontStyle,
    /// Font size in logical pixels.
    pub size: f32,
    /// Fill color for the glyphs.
    pub color: Color,
    /// Extra spacing between letters, in logical pixels.
    pub letter_spacing: f32,
    /// Line height.
    pub line_height: LineHeight,
}

impl TextStyle {
    /// A style with the given `size` and `color`; every other knob keeps its
    /// [`Default`] value.
    pub fn new(size: f32, color: Color) -> Self {
        Self {
            size,
            color,
            ..Self::default()
        }
    }
}

impl Default for TextStyle {
    /// System UI family, 400 weight, upright, 16px, opaque black, no extra
    /// letter-spacing, the font's natural line height.
    fn default() -> Self {
        Self {
            family: FontFamily::default(),
            weight: FontWeight::default(),
            style: FontStyle::default(),
            size: 16.0,
            color: Color::BLACK,
            letter_spacing: 0.0,
            line_height: LineHeight::default(),
        }
    }
}

/// Converts a Frust [`FontFamily`] into parley's owned `'static`
/// `FontFamily`. `pub(crate)` — parley types must not leak past the crate
/// boundary (scene-layer purity); shared by [`crate::context`] and
/// [`crate::editor`].
pub(crate) fn to_parley_family(family: &FontFamily) -> parley::FontFamily<'static> {
    match family {
        FontFamily::SystemUi => parley::GenericFamily::SystemUi.into(),
        FontFamily::Named(names) => {
            let list: Vec<parley::FontFamilyName<'static>> = names
                .iter()
                .map(|name| parley::FontFamilyName::Named(name.clone().into()))
                .collect();
            parley::FontFamily::List(list.into())
        }
    }
}

/// Converts a Frust [`FontWeight`] into parley's `FontWeight`. `pub(crate)`
/// — see [`to_parley_family`].
pub(crate) fn to_parley_weight(weight: FontWeight) -> parley::FontWeight {
    parley::FontWeight::new(weight.0)
}

/// Converts a Frust [`FontStyle`] into parley's `FontStyle`. `pub(crate)`
/// — see [`to_parley_family`].
pub(crate) fn to_parley_style(style: FontStyle) -> parley::FontStyle {
    match style {
        FontStyle::Normal => parley::FontStyle::Normal,
        FontStyle::Italic => parley::FontStyle::Italic,
        FontStyle::Oblique(angle) => parley::FontStyle::Oblique(angle),
    }
}

/// Converts a Frust [`LineHeight`] into parley's `LineHeight`.
/// `pub(crate)` — see [`to_parley_family`].
pub(crate) fn to_parley_line_height(line_height: LineHeight) -> parley::LineHeight {
    match line_height {
        LineHeight::MetricsRelative(v) => parley::LineHeight::MetricsRelative(v),
        LineHeight::FontSizeRelative(v) => parley::LineHeight::FontSizeRelative(v),
        LineHeight::Absolute(v) => parley::LineHeight::Absolute(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preserves_old_v1_semantics() {
        let style = TextStyle::default();
        assert_eq!(style.family, FontFamily::SystemUi);
        assert_eq!(style.weight, FontWeight::REGULAR);
        assert_eq!(style.style, FontStyle::Normal);
        assert_eq!(style.size, 16.0);
        assert_eq!(style.color, Color::BLACK);
        assert_eq!(style.letter_spacing, 0.0);
        assert_eq!(style.line_height, LineHeight::MetricsRelative(1.0));
    }

    #[test]
    fn new_only_overrides_size_and_color() {
        let style = TextStyle::new(24.0, Color::from_rgb8(0x11, 0x22, 0x33));
        assert_eq!(style.size, 24.0);
        assert_eq!(style.color, Color::from_rgb8(0x11, 0x22, 0x33));
        assert_eq!(style.family, FontFamily::SystemUi);
        assert_eq!(style.weight, FontWeight::REGULAR);
        assert_eq!(style.style, FontStyle::Normal);
    }

    #[test]
    fn font_weight_consts_match_css_scale() {
        assert_eq!(FontWeight::THIN.value(), 100.0);
        assert_eq!(FontWeight::REGULAR.value(), 400.0);
        assert_eq!(FontWeight::MEDIUM.value(), 500.0);
        assert_eq!(FontWeight::BOLD.value(), 700.0);
        assert_eq!(FontWeight::BLACK.value(), 900.0);
    }

    #[test]
    fn family_named_and_stack_constructors() {
        assert_eq!(
            FontFamily::named("Inter"),
            FontFamily::Named(vec!["Inter".to_string()])
        );
        assert_eq!(
            FontFamily::stack(["Inter", "Roboto"]),
            FontFamily::Named(vec!["Inter".to_string(), "Roboto".to_string()])
        );
    }
}
