//! Material 3 type scale: 15 named [`forgekit_text::TextStyle`] presets.
//!
//! Source: <https://m3.material.io/styles/typography/type-scale-tokens>
//! (verified 2026-07-17). Sizes are specified in sp; ForgeKit treats sp and
//! logical px 1:1 (see `forgekit-text`'s scale). Line heights are absolute
//! logical pixels (`LineHeight::Absolute`, not a font-size-relative ratio) —
//! M3 publishes them as fixed px values per token, not a ratio. Letter
//! spacing is in logical pixels; `displayLarge`'s spacing is negative
//! (tighter tracking at very large sizes).
//!
//! `titleLarge` is weight 400 (Regular) per m3.material.io; a secondary
//! source claims 500 — this module follows m3.material.io as the primary,
//! more authoritative source (see
//! `workflow/plans/features/forgekit-phase-6a-foundations/tasks/03-theme-crate-tokens.md`).

use forgekit_text::{FontWeight, LineHeight, TextStyle};

/// The 15 Material 3 type-scale presets, each a full [`TextStyle`].
///
/// Build one from a `base` style (its `family`/`style`/`color` are kept;
/// `size`/`weight`/`letter_spacing`/`line_height` are overridden per token)
/// via [`TypeScale::m3`].
#[derive(Clone, Debug, PartialEq)]
pub struct TypeScale {
    pub display_large: TextStyle,
    pub display_medium: TextStyle,
    pub display_small: TextStyle,
    pub headline_large: TextStyle,
    pub headline_medium: TextStyle,
    pub headline_small: TextStyle,
    pub title_large: TextStyle,
    pub title_medium: TextStyle,
    pub title_small: TextStyle,
    pub body_large: TextStyle,
    pub body_medium: TextStyle,
    pub body_small: TextStyle,
    pub label_large: TextStyle,
    pub label_medium: TextStyle,
    pub label_small: TextStyle,
}

/// One type-scale token's numeric shape: `(size_px, line_height_px,
/// letter_spacing_px, weight)`.
type Token = (f32, f32, f32, FontWeight);

const DISPLAY_LARGE: Token = (57.0, 64.0, -0.25, FontWeight::REGULAR);
const DISPLAY_MEDIUM: Token = (45.0, 52.0, 0.0, FontWeight::REGULAR);
const DISPLAY_SMALL: Token = (36.0, 44.0, 0.0, FontWeight::REGULAR);
const HEADLINE_LARGE: Token = (32.0, 40.0, 0.0, FontWeight::REGULAR);
const HEADLINE_MEDIUM: Token = (28.0, 36.0, 0.0, FontWeight::REGULAR);
const HEADLINE_SMALL: Token = (24.0, 32.0, 0.0, FontWeight::REGULAR);
const TITLE_LARGE: Token = (22.0, 28.0, 0.0, FontWeight::REGULAR);
const TITLE_MEDIUM: Token = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const TITLE_SMALL: Token = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const BODY_LARGE: Token = (16.0, 24.0, 0.5, FontWeight::REGULAR);
const BODY_MEDIUM: Token = (14.0, 20.0, 0.25, FontWeight::REGULAR);
const BODY_SMALL: Token = (12.0, 16.0, 0.4, FontWeight::REGULAR);
const LABEL_LARGE: Token = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const LABEL_MEDIUM: Token = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_SMALL: Token = (11.0, 16.0, 0.5, FontWeight::MEDIUM);

fn apply(base: &TextStyle, token: Token) -> TextStyle {
    let (size, line_height_px, letter_spacing, weight) = token;
    TextStyle {
        size,
        weight,
        letter_spacing,
        line_height: LineHeight::Absolute(line_height_px),
        ..base.clone()
    }
}

impl TypeScale {
    /// Builds the Material 3 baseline type scale from `base` (its
    /// `family`/`style`/`color` are preserved on every token; only
    /// `size`/`weight`/`letter_spacing`/`line_height` are M3-specified).
    pub fn m3(base: &TextStyle) -> Self {
        Self {
            display_large: apply(base, DISPLAY_LARGE),
            display_medium: apply(base, DISPLAY_MEDIUM),
            display_small: apply(base, DISPLAY_SMALL),
            headline_large: apply(base, HEADLINE_LARGE),
            headline_medium: apply(base, HEADLINE_MEDIUM),
            headline_small: apply(base, HEADLINE_SMALL),
            title_large: apply(base, TITLE_LARGE),
            title_medium: apply(base, TITLE_MEDIUM),
            title_small: apply(base, TITLE_SMALL),
            body_large: apply(base, BODY_LARGE),
            body_medium: apply(base, BODY_MEDIUM),
            body_small: apply(base, BODY_SMALL),
            label_large: apply(base, LABEL_LARGE),
            label_medium: apply(base, LABEL_MEDIUM),
            label_small: apply(base, LABEL_SMALL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::Color;

    #[test]
    fn display_large_matches_table() {
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large.size, 57.0);
        assert_eq!(scale.display_large.line_height, LineHeight::Absolute(64.0));
        assert_eq!(scale.display_large.letter_spacing, -0.25);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn title_medium_is_medium_weight() {
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_medium.weight, FontWeight::MEDIUM);
        assert_eq!(scale.title_medium.size, 16.0);
        assert_eq!(scale.title_medium.letter_spacing, 0.15);
    }

    #[test]
    fn title_large_is_regular_weight() {
        // m3.material.io: titleLarge is weight 400, not 500 (see module docs
        // for the contested secondary-source claim this rejects).
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn label_small_matches_table() {
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.label_small.size, 11.0);
        assert_eq!(scale.label_small.line_height, LineHeight::Absolute(16.0));
        assert_eq!(scale.label_small.letter_spacing, 0.5);
        assert_eq!(scale.label_small.weight, FontWeight::MEDIUM);
    }

    #[test]
    fn preserves_base_family_and_color() {
        let base = TextStyle {
            family: forgekit_text::FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = TypeScale::m3(&base);
        assert_eq!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.color, base.color);
    }
}
