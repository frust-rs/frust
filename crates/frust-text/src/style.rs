//! [`TextStyle`]: the styling knobs applied uniformly to a laid-out string —
//! family, weight, style (italic/oblique), width, variation axes, locale, size,
//! color, letter-spacing, and line-height (the M3 type-scale surface).
//!
//! Every type here is Frust-owned, not a re-export of a text engine's type
//! (scene-layer purity, see `docs/ARCHITECTURE.md`). [`FontFamily`]/
//! [`FontWeight`]/[`FontStyle`]/[`LineHeight`] keep parley 0.11's semantics;
//! the frust shaping driver (`crate::shape`) reads these types
//! directly and resolves [`LineHeight`] through [`LineHeight::resolve`], so no
//! shaping-engine type appears here. Rich per-range styling is deferred.

use peniko::Color;

/// One element of a fallback stack: a concrete family name or a generic class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FamilyName {
    /// A concrete named font family (e.g. "Inter", "IBM Plex Mono").
    Named(String),
    /// A generic fallback family (e.g. monospace, serif).
    Generic(GenericSlot),
}

/// Generic font families supported as fallback slots.
///
/// This is a curated subset of the CSS generic families covering the ones
/// Frust explicitly supports and exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenericSlot {
    /// A monospace (fixed-width) font family.
    Monospace,
    /// A sans-serif font family.
    SansSerif,
    /// A serif font family.
    Serif,
    /// The platform system UI font.
    SystemUi,
    /// Color emoji or symbol glyphs.
    Emoji,
}

/// Font family selection.
///
/// The default, [`FontFamily::SystemUi`], resolves to the platform UI font
/// (e.g. San Francisco on macOS) via the font oracle's platform backend with
/// no registration required. [`FontFamily::Named`] is an ordered fallback
/// stack of named families, tried in order; a name that doesn't resolve on
/// the current platform falls back per the oracle's fallback — no error
/// surface is exposed here. [`FontFamily::NamedWithGeneric`] extends
/// [`FontFamily::Named`] by allowing the stack to end with a generic
/// fallback family (e.g. monospace, serif).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum FontFamily {
    /// The platform system UI font. Default.
    #[default]
    SystemUi,
    /// An ordered fallback stack of named font families.
    Named(Vec<String>),
    /// An ordered fallback stack of named families ending in a generic fallback.
    NamedWithGeneric(Vec<FamilyName>),
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

    /// An ordered fallback stack of named font families ending in a generic
    /// fallback family.
    ///
    /// # Example
    ///
    /// ```
    /// use frust_text::{FontFamily, GenericSlot};
    ///
    /// // Try "IBM Plex Mono" first, fall back to system monospace
    /// let family = FontFamily::stack_with_generic(
    ///     ["IBM Plex Mono"],
    ///     GenericSlot::Monospace,
    /// );
    /// ```
    pub fn stack_with_generic(
        names: impl IntoIterator<Item = impl Into<String>>,
        generic: GenericSlot,
    ) -> Self {
        let mut families: Vec<FamilyName> = names
            .into_iter()
            .map(|name| FamilyName::Named(name.into()))
            .collect();
        families.push(FamilyName::Generic(generic));
        Self::NamedWithGeneric(families)
    }
}

/// Visual weight of a font, on the standard 1-1000 CSS `font-weight` scale.
///
/// The CSS weight scale fontique's `FontWeight` uses, without leaking that
/// type past this crate's boundary.
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
/// The CSS slants fontique's `FontStyle` names.
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

/// Paragraph alignment: how each line is positioned within the layout's
/// width.
///
/// parley 0.11's `Alignment` semantics, without its type. Only meaningful
/// when the layout is wrapped to a `max_width` ([`crate::TextContext::layout`]'s
/// third argument, or [`crate::TextEditor::set_wrap_width`]) — an unbounded
/// layout's line width already equals its content width, so every alignment
/// renders identically to [`TextAlign::Start`].
///
/// Applies to [`crate::TextContext::layout`] and to a live-edited
/// [`crate::TextEditor`] (the engine behind `TextInput`) alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextAlign {
    /// Leading edge of the line's text direction (left for LTR, right for
    /// RTL). Default — matches v1's hardcoded behavior exactly.
    #[default]
    Start,
    /// Trailing edge of the line's text direction (right for LTR, left for
    /// RTL).
    End,
    /// Always the left edge, regardless of text direction.
    Left,
    /// Each line centered within the layout's width.
    Center,
    /// Always the right edge, regardless of text direction.
    Right,
    /// Each line except the last is spaced out to fill the layout's width.
    Justify,
}

/// How text that overflows a bounded [`TextContext::layout_bounded`]
/// (`max_lines`) is handled.
///
/// Mirrors Flutter's `TextOverflow.clip`/`TextOverflow.ellipsis` shape (a
/// deliberately small subset — no `fade`/`visible`). Only meaningful
/// alongside `max_lines`: with no line cap, neither variant changes
/// anything (there is nothing to overflow past).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextOverflow {
    /// Drop whole lines past `max_lines` outright; the last visible line's
    /// own text is left exactly as the line-breaker assigned it, even if it
    /// is itself wider than the box (an unbreakable run with no wrap
    /// opportunity) — Frust does not character-trim in this mode. Default,
    /// and a no-op when `max_lines` is unset (today's behavior).
    #[default]
    Clip,
    /// Same line-dropping as [`Self::Clip`], plus an ellipsis on the last
    /// visible line. That line is refilled from the text after its
    /// soft-wrap point, up to the next mandatory break (or the end of the
    /// text), then cut at the longest grapheme-boundary prefix whose
    /// advance plus `'…'` fits the layout's `max_width`. A cut never ends
    /// on whitespace, and may fall inside a word wider than the line. If
    /// not even the bare `'…'` fits, it is drawn alone. With an unbounded
    /// `max_width` there is nothing to fit against, so the whole refilled
    /// extent is kept (minus trailing whitespace) with `'…'` appended.
    Ellipsis,
}

/// Line height: how much vertical space each line of text occupies.
///
/// parley 0.11's `LineHeight` semantics, without its type. M3's type scale specifies line heights
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

impl LineHeight {
    /// The line height in logical pixels for a run at `font_size` whose face
    /// has these metrics (`descent` as a positive distance below the
    /// baseline): parley 0.11's rule, so both paths size lines identically.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the line layout over the shaping driver is its first caller"
        )
    )]
    pub(crate) fn resolve(self, font_size: f32, ascent: f32, descent: f32, leading: f32) -> f32 {
        match self {
            Self::MetricsRelative(v) => (ascent + descent + leading) * v,
            Self::FontSizeRelative(v) => font_size * v,
            Self::Absolute(v) => v,
        }
    }
}

/// Lines drawn over a run of text: an underline and/or a line-through.
///
/// The lines are placed from the font's own underline and strikeout metrics
/// (see [`crate::TextLayout::decoration_rects`]); [`Self::with_thickness`]
/// overrides the stroke thickness and [`Self::with_color`] the colour, which
/// otherwise follows the text.
///
/// ```
/// use frust_text::TextDecoration;
///
/// let link = TextDecoration::underline();
/// assert!(link.underline && !link.line_through);
/// assert_eq!(TextDecoration::default(), TextDecoration::none());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TextDecoration {
    /// Draw a line under the text.
    pub underline: bool,
    /// Draw a line through the middle of the text.
    pub line_through: bool,
    /// The line colour; `None` follows the text colour.
    pub color: Option<Color>,
    /// The line thickness in logical pixels; `None` takes the font's metric.
    pub thickness: Option<f32>,
}

impl TextDecoration {
    /// No decoration. The [`Default`].
    pub fn none() -> Self {
        Self::default()
    }

    /// An underline.
    pub fn underline() -> Self {
        Self {
            underline: true,
            ..Self::default()
        }
    }

    /// A line through the text.
    pub fn line_through() -> Self {
        Self {
            line_through: true,
            ..Self::default()
        }
    }

    /// This decoration drawn in `color` rather than the text colour.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// This decoration with its lines `thickness` logical pixels thick.
    pub fn with_thickness(mut self, thickness: f32) -> Self {
        self.thickness = Some(thickness);
        self
    }

    /// Whether any line is drawn.
    pub fn is_none(&self) -> bool {
        !self.underline && !self.line_through
    }
}

/// What a line does with a word wider than itself (CSS `overflow-wrap`).
///
/// A paragraph-level knob, like [`TextAlign`]: the first span's style decides
/// for the whole layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OverflowWrap {
    /// A word with no break opportunity wider than the line overflows it (the
    /// default: a clipped or ellipsized long token).
    #[default]
    Normal,
    /// Such a word breaks before its first character that does not fit, so
    /// a URL, an identifier or a Thai word wider than a narrow column wraps
    /// instead of overflowing.
    Anywhere,
}

/// Styling applied uniformly to a laid-out string.
///
/// `TextStyle` is `#[non_exhaustive]`: a struct literal from outside this
/// crate does not compile, so build one with [`TextStyle::new`] or
/// [`TextStyle::default`] and chain the `with_*` builders. The fields stay
/// public and readable, and assignable on a mutable binding
/// (`style.size = 14.0`).
///
/// ```compile_fail,E0639
/// use frust_text::TextStyle;
/// use peniko::Color;
///
/// // Rejected: a struct literal of a non-exhaustive type from another crate.
/// let style = TextStyle {
///     size: 14.0,
///     color: Color::BLACK,
///     ..TextStyle::default()
/// };
/// ```
///
/// ```
/// use frust_text::{FontWeight, TextStyle};
/// use peniko::Color;
///
/// let style = TextStyle::new(14.0, Color::BLACK)
///     .with_weight(FontWeight::BOLD)
///     .with_letter_spacing(0.5);
/// assert_eq!(style.size, 14.0);
/// assert_eq!(style.weight, FontWeight::BOLD);
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextStyle {
    /// Font family (or fallback stack).
    pub family: FontFamily,
    /// Font weight.
    pub weight: FontWeight,
    /// Font style (upright/italic/oblique).
    pub style: FontStyle,
    /// Font width as a ratio of normal (`1.0`), the CSS `font-stretch` scale:
    /// a `wdth` axis takes it, otherwise the closest-width face is preferred.
    pub width: f32,
    /// Variation-axis settings (`(tag, value)`, e.g. `(*b"GRAD", 50.0)`),
    /// applied over whatever weight/width/slant synthesis chose, in order; a
    /// later setting of the same tag wins. Axes a face lacks are ignored.
    pub variations: Vec<([u8; 4], f32)>,
    /// OpenType feature settings (`(tag, value)`, e.g. `(*b"tnum", 1)` for
    /// tabular figures or `(*b"liga", 0)` to disable ligatures), in order; a
    /// later setting of the same tag wins. Features a face lacks are ignored.
    pub features: Vec<([u8; 4], u16)>,
    /// BCP 47 locale of the text (`"ja"`, `"zh-Hant"`, ...): steers font
    /// fallback, language-specific shaping and CJK line breaking.
    pub locale: Option<String>,
    /// Font size in logical pixels.
    pub size: f32,
    /// Fill color for the glyphs.
    pub color: Color,
    /// Extra spacing between letters, in logical pixels.
    pub letter_spacing: f32,
    /// Line height.
    pub line_height: LineHeight,
    /// Paragraph alignment. Defaults to [`TextAlign::Start`].
    pub align: TextAlign,
    /// What a word wider than the line does. Defaults to
    /// [`OverflowWrap::Normal`].
    pub overflow_wrap: OverflowWrap,
    /// Underline and line-through. Defaults to none.
    ///
    /// Decorations are painted by `TextView` (`text(..)`) only; a `TextInput`'s
    /// content and placeholder ignore this field.
    pub decoration: TextDecoration,
}

/// One piece of a multi-style paragraph: `text` drawn in `style`.
///
/// [`TextContext::layout_rich`](crate::TextContext::layout_rich) lays a slice
/// of spans out as one paragraph. The pieces are concatenated in order, so the
/// caller never states byte ranges. Paragraph-level properties
/// ([`TextStyle::align`], [`TextStyle::overflow_wrap`]) are read from the
/// first span's style; everything else applies per span.
#[derive(Clone, Copy, Debug)]
pub struct TextSpan<'a> {
    /// The piece of text.
    pub text: &'a str,
    /// The style the piece is drawn in.
    pub style: &'a TextStyle,
}

impl<'a> TextSpan<'a> {
    /// A span of `text` in `style`.
    pub fn new(text: &'a str, style: &'a TextStyle) -> Self {
        Self { text, style }
    }
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

    /// This style at font width `width` (ratio of normal, `1.0`).
    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// This style with one more variation-axis setting appended.
    pub fn with_variation(mut self, tag: [u8; 4], value: f32) -> Self {
        self.variations.push((tag, value));
        self
    }

    /// This style with its variation-axis settings replaced by `variations`.
    pub fn with_variations(mut self, variations: impl IntoIterator<Item = ([u8; 4], f32)>) -> Self {
        self.variations = variations.into_iter().collect();
        self
    }

    /// This style with one more OpenType feature setting appended (a later
    /// setting of the same tag wins).
    pub fn with_feature(mut self, tag: [u8; 4], value: u16) -> Self {
        self.features.push((tag, value));
        self
    }

    /// This style with its OpenType feature settings replaced by `features`
    /// (a later setting of the same tag wins).
    pub fn with_features(mut self, features: impl IntoIterator<Item = ([u8; 4], u16)>) -> Self {
        self.features = features.into_iter().collect();
        self
    }

    /// This style with the given [`OverflowWrap`] behavior.
    ///
    /// Paragraph-level: the first style decides; later spans' values are ignored.
    pub fn with_overflow_wrap(mut self, overflow_wrap: OverflowWrap) -> Self {
        self.overflow_wrap = overflow_wrap;
        self
    }

    /// This style with the given [`TextDecoration`].
    ///
    /// Painted by `TextView` (`text(..)`) only; a `TextInput`'s content and
    /// placeholder ignore the decoration.
    pub fn with_decoration(mut self, decoration: TextDecoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// This style with the BCP 47 `locale` (`None` clears it).
    pub fn with_locale(mut self, locale: Option<impl Into<String>>) -> Self {
        self.locale = locale.map(Into::into);
        self
    }

    /// This style with the given font `family` (or fallback stack).
    pub fn with_family(mut self, family: FontFamily) -> Self {
        self.family = family;
        self
    }

    /// This style with the given font `weight`.
    pub fn with_weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    /// This style with the given font `style` (upright, italic or oblique).
    pub fn with_style(mut self, style: FontStyle) -> Self {
        self.style = style;
        self
    }

    /// This style at font size `size`, in logical pixels.
    pub fn with_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// This style with the given glyph fill `color`.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// This style with extra letter spacing `letter_spacing`, in logical pixels.
    pub fn with_letter_spacing(mut self, letter_spacing: f32) -> Self {
        self.letter_spacing = letter_spacing;
        self
    }

    /// This style with the given [`LineHeight`].
    pub fn with_line_height(mut self, line_height: LineHeight) -> Self {
        self.line_height = line_height;
        self
    }

    /// This style with the given paragraph [`TextAlign`].
    pub fn with_align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }
}

impl Default for TextStyle {
    /// System UI family, 400 weight, upright, normal width, no variation
    /// settings or locale, 16px, opaque black, no extra letter-spacing, the
    /// font's natural line height.
    fn default() -> Self {
        Self {
            family: FontFamily::default(),
            weight: FontWeight::default(),
            style: FontStyle::default(),
            width: 1.0,
            variations: Vec::new(),
            features: Vec::new(),
            locale: None,
            size: 16.0,
            color: Color::BLACK,
            letter_spacing: 0.0,
            line_height: LineHeight::default(),
            align: TextAlign::default(),
            overflow_wrap: OverflowWrap::default(),
            decoration: TextDecoration::default(),
        }
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
        assert_eq!(style.align, TextAlign::Start);
        assert_eq!(style.width, 1.0);
        assert!(style.variations.is_empty());
        assert_eq!(style.locale, None);
    }

    #[test]
    fn builder_setters_fill_width_variations_and_locale() {
        let style = TextStyle::new(12.0, Color::BLACK)
            .with_width(0.75)
            .with_variation(*b"wght", 650.0)
            .with_variation(*b"GRAD", 50.0)
            .with_locale(Some("ja"));
        assert_eq!(style.width, 0.75);
        assert_eq!(style.variations, vec![(*b"wght", 650.0), (*b"GRAD", 50.0)]);
        assert_eq!(style.locale.as_deref(), Some("ja"));
        let style = style
            .with_variations([(*b"ROND", 100.0)])
            .with_locale(None::<String>);
        assert_eq!(style.variations, vec![(*b"ROND", 100.0)]);
        assert_eq!(style.locale, None);
    }

    #[test]
    fn every_field_has_a_builder() {
        let color = Color::from_rgb8(0x12, 0x34, 0x56);
        let style = TextStyle::default()
            .with_family(FontFamily::named("Inter"))
            .with_weight(FontWeight::BOLD)
            .with_style(FontStyle::Italic)
            .with_width(0.5)
            .with_variation(*b"GRAD", 50.0)
            .with_feature(*b"tnum", 1)
            .with_locale(Some("ja"))
            .with_size(20.0)
            .with_color(color)
            .with_letter_spacing(1.5)
            .with_line_height(LineHeight::Absolute(24.0))
            .with_align(TextAlign::Center)
            .with_overflow_wrap(OverflowWrap::Anywhere)
            .with_decoration(TextDecoration::underline().with_thickness(2.0));
        assert_eq!(style.family, FontFamily::Named(vec!["Inter".to_string()]));
        assert_eq!(style.weight, FontWeight::BOLD);
        assert_eq!(style.style, FontStyle::Italic);
        assert_eq!(style.width, 0.5);
        assert_eq!(style.variations, vec![(*b"GRAD", 50.0)]);
        assert_eq!(style.features, vec![(*b"tnum", 1)]);
        assert_eq!(style.locale.as_deref(), Some("ja"));
        assert_eq!(style.size, 20.0);
        assert_eq!(style.color, color);
        assert_eq!(style.letter_spacing, 1.5);
        assert_eq!(style.line_height, LineHeight::Absolute(24.0));
        assert_eq!(style.align, TextAlign::Center);
        assert_eq!(style.overflow_wrap, OverflowWrap::Anywhere);
        assert_eq!(
            style.decoration,
            TextDecoration::underline().with_thickness(2.0)
        );
    }

    #[test]
    fn builder_chain_equals_struct_literal() {
        let color = Color::from_rgb8(0x12, 0x34, 0x56);
        let literal = TextStyle {
            family: FontFamily::Named(vec!["Inter".to_string()]),
            weight: FontWeight::BOLD,
            style: FontStyle::Oblique(Some(12.0)),
            width: 0.75,
            variations: vec![(*b"wght", 650.0)],
            features: vec![(*b"liga", 0), (*b"ss01", 1)],
            locale: Some("zh-Hant".to_string()),
            size: 14.0,
            color,
            letter_spacing: 0.25,
            line_height: LineHeight::FontSizeRelative(1.5),
            align: TextAlign::Justify,
            overflow_wrap: OverflowWrap::Anywhere,
            decoration: TextDecoration::line_through().with_color(color),
        };
        let built = TextStyle::default()
            .with_family(FontFamily::named("Inter"))
            .with_weight(FontWeight::BOLD)
            .with_style(FontStyle::Oblique(Some(12.0)))
            .with_width(0.75)
            .with_variations([(*b"wght", 650.0)])
            .with_features([(*b"liga", 0), (*b"ss01", 1)])
            .with_locale(Some("zh-Hant"))
            .with_size(14.0)
            .with_color(color)
            .with_letter_spacing(0.25)
            .with_line_height(LineHeight::FontSizeRelative(1.5))
            .with_align(TextAlign::Justify)
            .with_overflow_wrap(OverflowWrap::Anywhere)
            .with_decoration(TextDecoration::line_through().with_color(color));
        assert_eq!(built, literal);
    }

    #[test]
    fn line_height_resolves_each_mode() {
        let (ascent, descent, leading) = (14.0, 4.0, 2.0);
        let resolve = |lh: LineHeight| lh.resolve(16.0, ascent, descent, leading);
        assert_eq!(resolve(LineHeight::MetricsRelative(1.0)), 20.0);
        assert_eq!(resolve(LineHeight::MetricsRelative(1.5)), 30.0);
        assert_eq!(resolve(LineHeight::FontSizeRelative(2.0)), 32.0);
        assert_eq!(resolve(LineHeight::Absolute(9.0)), 9.0);
    }

    #[test]
    fn new_only_overrides_size_and_color() {
        let style = TextStyle::new(24.0, Color::from_rgb8(0x11, 0x22, 0x33));
        assert_eq!(style.size, 24.0);
        assert_eq!(style.color, Color::from_rgb8(0x11, 0x22, 0x33));
        assert_eq!(style.family, FontFamily::SystemUi);
        assert_eq!(style.weight, FontWeight::REGULAR);
        assert_eq!(style.style, FontStyle::Normal);
        assert_eq!(style.align, TextAlign::Start);
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

    #[test]
    fn stack_with_generic_creates_correct_variant() {
        let family = FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace);
        match family {
            FontFamily::NamedWithGeneric(families) => {
                assert_eq!(families.len(), 2);
                assert_eq!(families[0], FamilyName::Named("IBM Plex Mono".to_string()));
                assert_eq!(families[1], FamilyName::Generic(GenericSlot::Monospace));
            }
            _ => panic!("Expected NamedWithGeneric variant"),
        }
    }
}
