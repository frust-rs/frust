//! [`TypeScale::neutral`]: the design-language-free type-scale progression —
//! 15 named [`frust_text::TextStyle`] presets plus 15 `_emphasized` siblings
//! (see "Emphasized type scale" below).
//!
//! This crate constructs no other `TypeScale` — a design system builds its
//! own from its own plugin crate (`frust-material`'s `tokens` module carries
//! the Material 3 mapping this table's numeric progression originates from,
//! citations included; `frust-cupertino`'s carries the Cupertino/SF Pro
//! mapping). The numeric progression below is duplicated into
//! `frust-material` deliberately: a type *scale* (a
//! size/weight/tracking/line-height ladder) isn't a branded design-system
//! artifact the way a named font face is, so [`TypeScale::neutral`] reuses
//! the identical numbers rather than inventing an unbranded ladder — only
//! `family` differs (see that constructor's doc comment).
//!
//! Source for the numbers themselves: Material 3's type-scale tokens
//! (<https://m3.material.io/styles/typography/type-scale-tokens>, verified
//! 2026-07-17). Sizes are specified in sp; Frust treats sp and logical px 1:1
//! (see `frust-text`'s scale). Line heights are absolute logical pixels
//! (`LineHeight::Absolute`, not a font-size-relative ratio) — M3 publishes
//! them as fixed px values per token, not a ratio. Letter spacing is in
//! logical pixels; `displayLarge`'s spacing is negative (tighter tracking at
//! very large sizes).
//!
//! `titleLarge` is weight 400 (Regular) per m3.material.io; a secondary
//! source claims 500 — this module follows m3.material.io as the primary,
//! more authoritative source.
//!
//! # Emphasized type scale
//!
//! [`TypeScale`] additionally carries 15 `_emphasized` variants (one per
//! baseline role, 30 slots total), filled by [`TypeScale::neutral`] alongside
//! the baseline 15 (`frust-material`/`frust-cupertino`'s own constructors do
//! the same for their own baselines).
//!
//! **Role-count resolution:** an earlier belief that emphasized variants were
//! "15 baseline + 15 emphasized (30 total), applied to Display/Headline/Title
//! roles" was contested — the "30 total" count was right but the
//! "Display/Headline/Title only" scope was wrong. Verified directly against
//! the primary source: Jetpack Compose Material3's generated token file
//! (`androidx.compose.material3.tokens.TypographyTokens`/`TypeScaleTokens`,
//! `VERSION: v0_103`,
//! <https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/TypeScaleTokens.kt>
//! — Compose's `Typography` class doc comments enumerate an
//! `*Emphasized` property for *all 15* baseline roles: `displayLarge`
//! through `labelSmall`, not a Display/Headline/Title-only subset;
//! retrieved/verified 2026-07-18). This module follows that: every one of
//! the 15 baseline roles gets an emphasized sibling.
//!
//! **M3 deltas** (from the same source): size and line height are unchanged
//! between a role's baseline and emphasized token — only weight (and, for a
//! handful of roles, letter spacing) shift. Weight always steps up one rung
//! from the baseline token's own weight: Regular → Medium for every
//! Regular-weight baseline role (`display_*`, `headline_*`, `title_large`,
//! `body_*`), and Medium → Bold for every Medium-weight baseline role
//! (`title_medium`, `title_small`, `label_*`) — so every emphasized style is
//! guaranteed to differ from its base in weight. Letter spacing mostly
//! matches the baseline value already in this module's tables; `body_large`
//! is the one role whose emphasized tracking differs from its own baseline
//! (0.5px baseline → 0.15px emphasized, matching the source's
//! `BodyLargeEmphasizedTracking`).

use frust_text::{FontFamily, FontWeight, GenericSlot, LineHeight, TextStyle};

/// The 15 named type-scale presets, each a full [`TextStyle`], plus 15
/// `_emphasized` siblings (see the module docs' "Emphasized type scale"
/// section).
///
/// Build one from a `base` style (its `family`/`style`/`color` are kept;
/// `size`/`weight`/`letter_spacing`/`line_height` are overridden per token)
/// via [`TypeScale::neutral`] — the only constructor this crate carries; a
/// design system builds its own from its own plugin crate (see the module
/// docs).
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
    pub display_large_emphasized: TextStyle,
    pub display_medium_emphasized: TextStyle,
    pub display_small_emphasized: TextStyle,
    pub headline_large_emphasized: TextStyle,
    pub headline_medium_emphasized: TextStyle,
    pub headline_small_emphasized: TextStyle,
    pub title_large_emphasized: TextStyle,
    pub title_medium_emphasized: TextStyle,
    pub title_small_emphasized: TextStyle,
    pub body_large_emphasized: TextStyle,
    pub body_medium_emphasized: TextStyle,
    pub body_small_emphasized: TextStyle,
    pub label_large_emphasized: TextStyle,
    pub label_medium_emphasized: TextStyle,
    pub label_small_emphasized: TextStyle,
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

// M3-Expressive emphasized tokens: same size/line-height as the matching
// baseline `Token` above in every case; weight steps up one rung from the
// baseline role's own weight (Regular -> Medium, Medium -> Bold) and letter
// spacing is the source's `*Emphasized*Tracking` value (see the module
// docs' "Emphasized type scale" section for the primary-source citation and
// resolution of the contested Display/Headline/Title-only scope claim).
const DISPLAY_LARGE_EMPHASIZED: Token = (57.0, 64.0, 0.0, FontWeight::MEDIUM);
const DISPLAY_MEDIUM_EMPHASIZED: Token = (45.0, 52.0, 0.0, FontWeight::MEDIUM);
const DISPLAY_SMALL_EMPHASIZED: Token = (36.0, 44.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_LARGE_EMPHASIZED: Token = (32.0, 40.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_MEDIUM_EMPHASIZED: Token = (28.0, 36.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_SMALL_EMPHASIZED: Token = (24.0, 32.0, 0.0, FontWeight::MEDIUM);
const TITLE_LARGE_EMPHASIZED: Token = (22.0, 28.0, 0.0, FontWeight::MEDIUM);
const TITLE_MEDIUM_EMPHASIZED: Token = (16.0, 24.0, 0.15, FontWeight::BOLD);
const TITLE_SMALL_EMPHASIZED: Token = (14.0, 20.0, 0.1, FontWeight::BOLD);
const BODY_LARGE_EMPHASIZED: Token = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const BODY_MEDIUM_EMPHASIZED: Token = (14.0, 20.0, 0.25, FontWeight::MEDIUM);
const BODY_SMALL_EMPHASIZED: Token = (12.0, 16.0, 0.4, FontWeight::MEDIUM);
const LABEL_LARGE_EMPHASIZED: Token = (14.0, 20.0, 0.1, FontWeight::BOLD);
const LABEL_MEDIUM_EMPHASIZED: Token = (12.0, 16.0, 0.5, FontWeight::BOLD);
const LABEL_SMALL_EMPHASIZED: Token = (11.0, 16.0, 0.5, FontWeight::BOLD);

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
    /// Builds the neutral, design-language-free type scale from `base` (its
    /// `style`/`color` are preserved).
    ///
    /// Reuses the same numeric size/weight/tracking/line-height progression
    /// `frust-material`'s M3-sourced type scale carries (see the module
    /// docs) — a type *scale* isn't a branded artifact the way a named font
    /// face is, so there's no design-language-specific value here to
    /// invent (see `crate::theme::Theme::neutral`'s module docs). The one
    /// thing this constructor overrides is `family`: every slot forces a
    /// generic sans-serif stack via [`FontFamily::stack_with_generic`] (no
    /// named font at all, ending in [`GenericSlot::SansSerif`]) rather than
    /// inheriting whatever `base` supplies — **no bundled font bytes are
    /// referenced**, so this compiles and behaves identically whether or not
    /// the `glyph-fonts` feature is enabled.
    pub fn neutral(base: &TextStyle) -> Self {
        let family =
            FontFamily::stack_with_generic(std::iter::empty::<&str>(), GenericSlot::SansSerif);
        let base = TextStyle {
            family,
            ..base.clone()
        };
        Self {
            display_large: apply(&base, DISPLAY_LARGE),
            display_medium: apply(&base, DISPLAY_MEDIUM),
            display_small: apply(&base, DISPLAY_SMALL),
            headline_large: apply(&base, HEADLINE_LARGE),
            headline_medium: apply(&base, HEADLINE_MEDIUM),
            headline_small: apply(&base, HEADLINE_SMALL),
            title_large: apply(&base, TITLE_LARGE),
            title_medium: apply(&base, TITLE_MEDIUM),
            title_small: apply(&base, TITLE_SMALL),
            body_large: apply(&base, BODY_LARGE),
            body_medium: apply(&base, BODY_MEDIUM),
            body_small: apply(&base, BODY_SMALL),
            label_large: apply(&base, LABEL_LARGE),
            label_medium: apply(&base, LABEL_MEDIUM),
            label_small: apply(&base, LABEL_SMALL),
            display_large_emphasized: apply(&base, DISPLAY_LARGE_EMPHASIZED),
            display_medium_emphasized: apply(&base, DISPLAY_MEDIUM_EMPHASIZED),
            display_small_emphasized: apply(&base, DISPLAY_SMALL_EMPHASIZED),
            headline_large_emphasized: apply(&base, HEADLINE_LARGE_EMPHASIZED),
            headline_medium_emphasized: apply(&base, HEADLINE_MEDIUM_EMPHASIZED),
            headline_small_emphasized: apply(&base, HEADLINE_SMALL_EMPHASIZED),
            title_large_emphasized: apply(&base, TITLE_LARGE_EMPHASIZED),
            title_medium_emphasized: apply(&base, TITLE_MEDIUM_EMPHASIZED),
            title_small_emphasized: apply(&base, TITLE_SMALL_EMPHASIZED),
            body_large_emphasized: apply(&base, BODY_LARGE_EMPHASIZED),
            body_medium_emphasized: apply(&base, BODY_MEDIUM_EMPHASIZED),
            body_small_emphasized: apply(&base, BODY_SMALL_EMPHASIZED),
            label_large_emphasized: apply(&base, LABEL_LARGE_EMPHASIZED),
            label_medium_emphasized: apply(&base, LABEL_MEDIUM_EMPHASIZED),
            label_small_emphasized: apply(&base, LABEL_SMALL_EMPHASIZED),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::Color;

    #[test]
    fn display_large_matches_table() {
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large.size, 57.0);
        assert_eq!(scale.display_large.line_height, LineHeight::Absolute(64.0));
        assert_eq!(scale.display_large.letter_spacing, -0.25);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn title_medium_is_medium_weight() {
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_medium.weight, FontWeight::MEDIUM);
        assert_eq!(scale.title_medium.size, 16.0);
        assert_eq!(scale.title_medium.letter_spacing, 0.15);
    }

    #[test]
    fn title_large_is_regular_weight() {
        // m3.material.io: titleLarge is weight 400, not 500 (see module docs
        // for the contested secondary-source claim this rejects).
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn label_small_matches_table() {
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.label_small.size, 11.0);
        assert_eq!(scale.label_small.line_height, LineHeight::Absolute(16.0));
        assert_eq!(scale.label_small.letter_spacing, 0.5);
        assert_eq!(scale.label_small.weight, FontWeight::MEDIUM);
    }

    /// Every emphasized role differs from its own baseline in weight — the
    /// "spec'd dimension" per the module docs' M3-deltas paragraph (weight
    /// always steps up one rung: Regular -> Medium or Medium -> Bold),
    /// verified across all 15 roles, not just Display/Headline/Title (the
    /// resolved, previously-contested scope question — see module docs).
    #[test]
    fn every_emphasized_role_differs_in_weight_from_its_base() {
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        let pairs: [(&TextStyle, &TextStyle); 15] = [
            (&scale.display_large, &scale.display_large_emphasized),
            (&scale.display_medium, &scale.display_medium_emphasized),
            (&scale.display_small, &scale.display_small_emphasized),
            (&scale.headline_large, &scale.headline_large_emphasized),
            (&scale.headline_medium, &scale.headline_medium_emphasized),
            (&scale.headline_small, &scale.headline_small_emphasized),
            (&scale.title_large, &scale.title_large_emphasized),
            (&scale.title_medium, &scale.title_medium_emphasized),
            (&scale.title_small, &scale.title_small_emphasized),
            (&scale.body_large, &scale.body_large_emphasized),
            (&scale.body_medium, &scale.body_medium_emphasized),
            (&scale.body_small, &scale.body_small_emphasized),
            (&scale.label_large, &scale.label_large_emphasized),
            (&scale.label_medium, &scale.label_medium_emphasized),
            (&scale.label_small, &scale.label_small_emphasized),
        ];
        for (base, emphasized) in pairs {
            assert_ne!(
                base.weight, emphasized.weight,
                "expected emphasized weight to differ from base weight"
            );
        }
    }

    #[test]
    fn emphasized_keeps_base_size_and_line_height() {
        // Only weight (and, for a few roles, letter spacing) shift between
        // a role's baseline and emphasized token — size and line height
        // are unchanged (see module docs' M3-deltas paragraph).
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(
            scale.display_large.size,
            scale.display_large_emphasized.size
        );
        assert_eq!(
            scale.display_large.line_height,
            scale.display_large_emphasized.line_height
        );
        assert_eq!(scale.label_small.size, scale.label_small_emphasized.size);
        assert_eq!(
            scale.label_small.line_height,
            scale.label_small_emphasized.line_height
        );
    }

    #[test]
    fn display_large_emphasized_is_medium_weight() {
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large_emphasized.weight, FontWeight::MEDIUM);
        assert_eq!(scale.display_large_emphasized.letter_spacing, 0.0);
    }

    #[test]
    fn title_medium_emphasized_is_bold_weight() {
        // title_medium's baseline weight is already Medium (500), so its
        // emphasized sibling steps up to Bold (700), not Medium again.
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_medium.weight, FontWeight::MEDIUM);
        assert_eq!(scale.title_medium_emphasized.weight, FontWeight::BOLD);
    }

    #[test]
    fn label_small_emphasized_is_bold_weight() {
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.label_small.weight, FontWeight::MEDIUM);
        assert_eq!(scale.label_small_emphasized.weight, FontWeight::BOLD);
    }

    #[test]
    fn body_large_emphasized_tracking_differs_from_base() {
        // body_large is the one role whose emphasized letter spacing also
        // differs from its own baseline (0.5px -> 0.15px), per the primary
        // source's BodyLargeEmphasizedTracking (see module docs).
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.body_large.letter_spacing, 0.5);
        assert_eq!(scale.body_large_emphasized.letter_spacing, 0.15);
    }

    // ---- Neutral scale ----------------------------------------------------

    #[test]
    fn neutral_uses_no_bundled_fonts() {
        // Every slot names only a generic/system family, never "Space
        // Mono"/"IBM Plex Mono" or any other concrete named font.
        let scale = TypeScale::neutral(&TextStyle::new(16.0, Color::BLACK));
        let expected =
            FontFamily::stack_with_generic(std::iter::empty::<&str>(), GenericSlot::SansSerif);
        for family in [
            &scale.display_large.family,
            &scale.headline_medium.family,
            &scale.title_large.family,
            &scale.body_large.family,
            &scale.label_small.family,
            &scale.body_large_emphasized.family,
        ] {
            assert_eq!(family, &expected);
            match family {
                FontFamily::NamedWithGeneric(parts) => {
                    // No `FamilyName::Named` entry — a generic-only stack.
                    assert!(
                        !parts
                            .iter()
                            .any(|p| matches!(p, frust_text::FamilyName::Named(_)))
                    );
                }
                other => panic!("expected a generic-only stack, got {other:?}"),
            }
        }
    }

    #[test]
    fn neutral_preserves_base_color_but_overrides_family() {
        let base = TextStyle {
            family: FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = TypeScale::neutral(&base);
        assert_ne!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.color, base.color);
    }
}
