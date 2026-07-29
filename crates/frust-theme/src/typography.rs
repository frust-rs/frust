//! Material 3 type scale: 15 named [`frust_text::TextStyle`] presets.
//!
//! Source: <https://m3.material.io/styles/typography/type-scale-tokens>
//! (verified 2026-07-17). Sizes are specified in sp; Frust treats sp and
//! logical px 1:1 (see `frust-text`'s scale). Line heights are absolute
//! logical pixels (`LineHeight::Absolute`, not a font-size-relative ratio) —
//! M3 publishes them as fixed px values per token, not a ratio. Letter
//! spacing is in logical pixels; `displayLarge`'s spacing is negative
//! (tighter tracking at very large sizes).
//!
//! `titleLarge` is weight 400 (Regular) per m3.material.io; a secondary
//! source claims 500 — this module follows m3.material.io as the primary,
//! more authoritative source.
//!
//! # Cupertino (iOS) mapping
//!
//! [`TypeScale::cupertino`] maps Apple's 11 SF Pro semantic text styles onto
//! the same 15 M3-named `TypeScale` slots above (retrieved 2026-07-17):
//!
//! - **Sizes** (pt, treated 1:1 as logical px like the M3 scale's sp): Large
//!   Title 34, Title 1 28, Title 2 22, Title 3 20, Headline 17, Body 17,
//!   Callout 16, Subheadline 15, Footnote 13, Caption 1 12, Caption 2 11.
//! - **Weights (R19 correction):** Apple's default text-style weight is
//!   Regular for every style *except* Headline, which is Semibold — Large
//!   Title/Title 1/Title 2/Title 3 are Regular by default, not Bold/Semibold
//!   as an earlier community table claimed; Bold/Semibold variants of those
//!   styles exist only as an opt-in "emphasized" style, not the default.
//! - **Family (R20 correction):** SF Pro automatically switches between the
//!   "Text" optical size (more open spacing, ≤19pt) and "Display" optical
//!   size (tighter spacing, ≥20pt) — driven purely by the requested point
//!   size, with **no dpi/density component** (an earlier community claim of
//!   a "144dpi" cutoff is not supported by any Apple documentation). This
//!   module encodes the cutoff by choosing a `FontFamily::stack(["SF Pro
//!   Display", …])`/`stack(["SF Pro Text", …])` per slot, keyed on that
//!   slot's own point size. **SF Pro's family-name resolution is
//!   compile-verified only in this task** — parley/fontique resolves a named
//!   family against the *current platform's* installed fonts, so "SF Pro
//!   Text"/"SF Pro Display" only actually resolve on iOS (SF Pro's license
//!   forbids cross-platform bundling, mirroring Flutter's Cupertino
//!   precedent of a system-font proxy that falls back off-Apple-platforms);
//!   on every other platform this stack falls back to **parley's own
//!   fallback font resolution**, not a "system UI font" per se — actual
//!   on-device resolution is a later phase's (6e) runtime concern, not this
//!   one's.
//! - **Letter spacing:** left at `0.0` for every Cupertino token — SF Pro's
//!   per-size optical tracking is baked into the font's own metrics tables
//!   (applied by CoreText/parley from the font itself), not an
//!   app-superimposed additional value the way M3's type scale specifies
//!   one.
//! - **Line height:** Apple's official HIG leading-per-style table is only
//!   partly confirmed (R19's correction flags some of the commonly-cited
//!   leading figures — Callout/Footnote/Caption1/Caption2 — as diverging
//!   from the archived official table); the values below are the
//!   widely-cited **community-approximate** absolute leading (pt) per style,
//!   not a source Apple itself currently publishes verbatim.
//! - **Slot mapping:** 11 source styles must fill 15 M3 slots, so 4 slots
//!   necessarily reuse an adjacent style's exact size/weight/leading — a
//!   Frust editorial choice (documented here, not implying a real 1:1
//!   Apple/M3 crosswalk exists) rather than an invented intermediate size:
//!
//!   | M3 slot          | SF Pro style   | size | weight   |
//!   |------------------|----------------|------|----------|
//!   | `display_large`  | Large Title    | 34   | Regular  |
//!   | `display_medium` | Large Title    | 34   | Regular  |
//!   | `display_small`  | Title 1        | 28   | Regular  |
//!   | `headline_large` | Title 1        | 28   | Regular  |
//!   | `headline_medium`| Title 2        | 22   | Regular  |
//!   | `headline_small` | Title 3        | 20   | Regular  |
//!   | `title_large`    | Headline       | 17   | Semibold |
//!   | `title_medium`   | Headline       | 17   | Semibold |
//!   | `title_small`    | Body           | 17   | Regular  |
//!   | `body_large`     | Body           | 17   | Regular  |
//!   | `body_medium`    | Callout        | 16   | Regular  |
//!   | `body_small`     | Subheadline    | 15   | Regular  |
//!   | `label_large`    | Footnote       | 13   | Regular  |
//!   | `label_medium`   | Caption 1      | 12   | Regular  |
//!   | `label_small`    | Caption 2      | 11   | Regular  |
//!
//!   The Text/Display family cutoff falls exactly at the `headline`/`title`
//!   boundary above (20pt vs 17pt), so every `display_*`/`headline_*` slot
//!   uses the Display stack and every `title_*`/`body_*`/`label_*` slot uses
//!   the Text stack.
//!
//! # Emphasized type scale (M3 Expressive)
//!
//! [`TypeScale`] additionally carries 15 `_emphasized` variants (one per
//! baseline role, 30 slots total), filled by [`TypeScale::m3`]/
//! [`TypeScale::cupertino`] alongside the baseline 15.
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
//!
//! **Cupertino mapping:** SF Pro weights mined from the Apple iOS UI Kit
//! (`text_styles` — only Regular and
//! Semibold appear), [`TypeScale::cupertino`]'s emphasized slots reuse the
//! same size/line-height/family as their baseline counterpart and force
//! weight to Semibold — including `title_large`/`title_medium` (mapped from
//! SF Pro Headline, already Semibold by default), whose emphasized variant is
//! therefore numerically identical to its own baseline; this is a faithful
//! mapping outcome (SF Pro has no weight above Semibold to step up to here),
//! not an oversight.

use frust_text::{FontFamily, FontWeight, LineHeight, TextStyle};

/// The 15 Material 3 type-scale presets, each a full [`TextStyle`], plus 15
/// M3-Expressive `_emphasized` siblings (see the module docs' "Emphasized
/// type scale" section).
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
            display_large_emphasized: apply(base, DISPLAY_LARGE_EMPHASIZED),
            display_medium_emphasized: apply(base, DISPLAY_MEDIUM_EMPHASIZED),
            display_small_emphasized: apply(base, DISPLAY_SMALL_EMPHASIZED),
            headline_large_emphasized: apply(base, HEADLINE_LARGE_EMPHASIZED),
            headline_medium_emphasized: apply(base, HEADLINE_MEDIUM_EMPHASIZED),
            headline_small_emphasized: apply(base, HEADLINE_SMALL_EMPHASIZED),
            title_large_emphasized: apply(base, TITLE_LARGE_EMPHASIZED),
            title_medium_emphasized: apply(base, TITLE_MEDIUM_EMPHASIZED),
            title_small_emphasized: apply(base, TITLE_SMALL_EMPHASIZED),
            body_large_emphasized: apply(base, BODY_LARGE_EMPHASIZED),
            body_medium_emphasized: apply(base, BODY_MEDIUM_EMPHASIZED),
            body_small_emphasized: apply(base, BODY_SMALL_EMPHASIZED),
            label_large_emphasized: apply(base, LABEL_LARGE_EMPHASIZED),
            label_medium_emphasized: apply(base, LABEL_MEDIUM_EMPHASIZED),
            label_small_emphasized: apply(base, LABEL_SMALL_EMPHASIZED),
        }
    }

    /// Builds the Cupertino (iOS) type scale from `base` (its `style`/`color`
    /// are preserved on every token; `size`/`weight`/`letter_spacing`
    /// (always `0.0`)/`line_height`/`family` are all Cupertino-specified —
    /// unlike [`TypeScale::m3`], `family` is overridden per slot too, since
    /// the Text/Display optical-size switch (see module docs) needs to be
    /// chosen per token, not inherited from `base`). See the module docs'
    /// "Cupertino (iOS) mapping" section for the full source table.
    pub fn cupertino(base: &TextStyle) -> Self {
        Self {
            display_large: apply_cupertino(base, CUPERTINO_DISPLAY_LARGE),
            display_medium: apply_cupertino(base, CUPERTINO_DISPLAY_MEDIUM),
            display_small: apply_cupertino(base, CUPERTINO_DISPLAY_SMALL),
            headline_large: apply_cupertino(base, CUPERTINO_HEADLINE_LARGE),
            headline_medium: apply_cupertino(base, CUPERTINO_HEADLINE_MEDIUM),
            headline_small: apply_cupertino(base, CUPERTINO_HEADLINE_SMALL),
            title_large: apply_cupertino(base, CUPERTINO_TITLE_LARGE),
            title_medium: apply_cupertino(base, CUPERTINO_TITLE_MEDIUM),
            title_small: apply_cupertino(base, CUPERTINO_TITLE_SMALL),
            body_large: apply_cupertino(base, CUPERTINO_BODY_LARGE),
            body_medium: apply_cupertino(base, CUPERTINO_BODY_MEDIUM),
            body_small: apply_cupertino(base, CUPERTINO_BODY_SMALL),
            label_large: apply_cupertino(base, CUPERTINO_LABEL_LARGE),
            label_medium: apply_cupertino(base, CUPERTINO_LABEL_MEDIUM),
            label_small: apply_cupertino(base, CUPERTINO_LABEL_SMALL),
            display_large_emphasized: apply_cupertino_emphasized(base, CUPERTINO_DISPLAY_LARGE),
            display_medium_emphasized: apply_cupertino_emphasized(base, CUPERTINO_DISPLAY_MEDIUM),
            display_small_emphasized: apply_cupertino_emphasized(base, CUPERTINO_DISPLAY_SMALL),
            headline_large_emphasized: apply_cupertino_emphasized(base, CUPERTINO_HEADLINE_LARGE),
            headline_medium_emphasized: apply_cupertino_emphasized(base, CUPERTINO_HEADLINE_MEDIUM),
            headline_small_emphasized: apply_cupertino_emphasized(base, CUPERTINO_HEADLINE_SMALL),
            title_large_emphasized: apply_cupertino_emphasized(base, CUPERTINO_TITLE_LARGE),
            title_medium_emphasized: apply_cupertino_emphasized(base, CUPERTINO_TITLE_MEDIUM),
            title_small_emphasized: apply_cupertino_emphasized(base, CUPERTINO_TITLE_SMALL),
            body_large_emphasized: apply_cupertino_emphasized(base, CUPERTINO_BODY_LARGE),
            body_medium_emphasized: apply_cupertino_emphasized(base, CUPERTINO_BODY_MEDIUM),
            body_small_emphasized: apply_cupertino_emphasized(base, CUPERTINO_BODY_SMALL),
            label_large_emphasized: apply_cupertino_emphasized(base, CUPERTINO_LABEL_LARGE),
            label_medium_emphasized: apply_cupertino_emphasized(base, CUPERTINO_LABEL_MEDIUM),
            label_small_emphasized: apply_cupertino_emphasized(base, CUPERTINO_LABEL_SMALL),
        }
    }
}

/// SF Pro's Text/Display optical-size cutoff, in pt: sizes `< 20.0` use "SF
/// Pro Text", sizes `>= 20.0` use "SF Pro Display" — point-size driven only,
/// no dpi component (R20 correction; see module docs).
const CUPERTINO_TEXT_DISPLAY_CUTOFF_PT: f32 = 20.0;

/// One Cupertino type-scale token's numeric shape: `(size_pt,
/// line_height_pt, weight)`. Letter spacing is always `0.0` (see module
/// docs); family is derived from `size_pt` against
/// [`CUPERTINO_TEXT_DISPLAY_CUTOFF_PT`], not stored here.
type CupertinoToken = (f32, f32, FontWeight);

// SF Pro semantic text styles used above the M3 slots (see the module docs'
// mapping table): sizes/weights per Apple's official defaults (R19
// correction — only Headline is Semibold); line heights are the
// widely-cited **community-approximate** absolute leading per style (R19
// flags some of these as unconfirmed against the archived official table).
const CUPERTINO_DISPLAY_LARGE: CupertinoToken = (34.0, 41.0, FontWeight::REGULAR); // Large Title
const CUPERTINO_DISPLAY_MEDIUM: CupertinoToken = (34.0, 41.0, FontWeight::REGULAR); // Large Title
const CUPERTINO_DISPLAY_SMALL: CupertinoToken = (28.0, 34.0, FontWeight::REGULAR); // Title 1
const CUPERTINO_HEADLINE_LARGE: CupertinoToken = (28.0, 34.0, FontWeight::REGULAR); // Title 1
const CUPERTINO_HEADLINE_MEDIUM: CupertinoToken = (22.0, 28.0, FontWeight::REGULAR); // Title 2
const CUPERTINO_HEADLINE_SMALL: CupertinoToken = (20.0, 25.0, FontWeight::REGULAR); // Title 3
const CUPERTINO_TITLE_LARGE: CupertinoToken = (17.0, 22.0, FontWeight::SEMI_BOLD); // Headline
const CUPERTINO_TITLE_MEDIUM: CupertinoToken = (17.0, 22.0, FontWeight::SEMI_BOLD); // Headline
const CUPERTINO_TITLE_SMALL: CupertinoToken = (17.0, 22.0, FontWeight::REGULAR); // Body
const CUPERTINO_BODY_LARGE: CupertinoToken = (17.0, 22.0, FontWeight::REGULAR); // Body
const CUPERTINO_BODY_MEDIUM: CupertinoToken = (16.0, 21.0, FontWeight::REGULAR); // Callout
const CUPERTINO_BODY_SMALL: CupertinoToken = (15.0, 20.0, FontWeight::REGULAR); // Subheadline
const CUPERTINO_LABEL_LARGE: CupertinoToken = (13.0, 18.0, FontWeight::REGULAR); // Footnote
const CUPERTINO_LABEL_MEDIUM: CupertinoToken = (12.0, 16.0, FontWeight::REGULAR); // Caption 1
const CUPERTINO_LABEL_SMALL: CupertinoToken = (11.0, 13.0, FontWeight::REGULAR); // Caption 2

/// The named SF Pro family stack for `size_pt`, switching at
/// [`CUPERTINO_TEXT_DISPLAY_CUTOFF_PT`] (see module docs). Resolution is
/// compile-verified only here — actual on-device font resolution is a later
/// phase's (6e) concern.
fn sf_family_for_size(size_pt: f32) -> FontFamily {
    if size_pt >= CUPERTINO_TEXT_DISPLAY_CUTOFF_PT {
        FontFamily::stack(["SF Pro Display", "SF Pro"])
    } else {
        FontFamily::stack(["SF Pro Text", "SF Pro"])
    }
}

fn apply_cupertino(base: &TextStyle, token: CupertinoToken) -> TextStyle {
    let (size, line_height_pt, weight) = token;
    TextStyle {
        family: sf_family_for_size(size),
        size,
        weight,
        letter_spacing: 0.0,
        line_height: LineHeight::Absolute(line_height_pt),
        ..base.clone()
    }
}

/// Builds a Cupertino emphasized token from its baseline `token`'s
/// size/line-height (unchanged), forcing weight to Semibold — the only
/// emphasized weight SF Pro's mined text styles (Regular, Semibold) support
/// (see the module docs' "Cupertino mapping" paragraph).
fn apply_cupertino_emphasized(base: &TextStyle, token: CupertinoToken) -> TextStyle {
    let (size, line_height_pt, _weight) = token;
    apply_cupertino(base, (size, line_height_pt, FontWeight::SEMI_BOLD))
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
            family: frust_text::FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = TypeScale::m3(&base);
        assert_eq!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.color, base.color);
    }

    #[test]
    fn cupertino_display_large_matches_large_title() {
        let scale = TypeScale::cupertino(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large.size, 34.0);
        assert_eq!(scale.display_large.line_height, LineHeight::Absolute(41.0));
        assert_eq!(scale.display_large.letter_spacing, 0.0);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn cupertino_only_title_slots_are_semibold() {
        // R19 correction: only Headline defaults to Semibold; every other
        // SF Pro style (including Large Title/Title 1/2/3) is Regular.
        let scale = TypeScale::cupertino(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_large.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.title_medium.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
        assert_eq!(scale.headline_small.weight, FontWeight::REGULAR);
        assert_eq!(scale.title_small.weight, FontWeight::REGULAR);
        assert_eq!(scale.body_large.weight, FontWeight::REGULAR);
        assert_eq!(scale.label_small.weight, FontWeight::REGULAR);
    }

    #[test]
    fn cupertino_family_switches_at_the_text_display_cutoff() {
        // R20: the Text/Display switch is size-driven — >= 20pt uses
        // Display, < 20pt uses Text — with no dpi component.
        let scale = TypeScale::cupertino(&TextStyle::new(16.0, Color::BLACK));
        let display_stack = frust_text::FontFamily::stack(["SF Pro Display", "SF Pro"]);
        let text_stack = frust_text::FontFamily::stack(["SF Pro Text", "SF Pro"]);

        assert_eq!(scale.display_large.family, display_stack); // 34pt
        assert_eq!(scale.headline_small.family, display_stack); // 20pt: boundary, Display
        assert_eq!(scale.title_large.family, text_stack); // 17pt
        assert_eq!(scale.label_small.family, text_stack); // 11pt
    }

    #[test]
    fn cupertino_preserves_base_style_and_color_but_overrides_family() {
        // Unlike TypeScale::m3, family is Cupertino-chosen per slot even
        // when `base` supplies its own (see this module's doc comment).
        let base = TextStyle {
            family: frust_text::FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = TypeScale::cupertino(&base);
        assert_ne!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.color, base.color);
        assert_eq!(scale.body_large.style, base.style);
    }

    /// Every M3 emphasized role differs from its own baseline in weight —
    /// the "spec'd dimension" per the module docs' M3-deltas paragraph
    /// (weight always steps up one rung: Regular -> Medium or Medium ->
    /// Bold), verified across all 15 roles, not just Display/Headline/Title
    /// (the resolved, previously-contested scope question — see module
    /// docs).
    #[test]
    fn m3_every_emphasized_role_differs_in_weight_from_its_base() {
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
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
    fn m3_emphasized_keeps_base_size_and_line_height() {
        // Only weight (and, for a few roles, letter spacing) shift between
        // an M3 role's baseline and emphasized token — size and line height
        // are unchanged (see module docs' M3-deltas paragraph).
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
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
    fn m3_display_large_emphasized_is_medium_weight() {
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large_emphasized.weight, FontWeight::MEDIUM);
        assert_eq!(scale.display_large_emphasized.letter_spacing, 0.0);
    }

    #[test]
    fn m3_title_medium_emphasized_is_bold_weight() {
        // title_medium's baseline weight is already Medium (500), so its
        // emphasized sibling steps up to Bold (700), not Medium again.
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_medium.weight, FontWeight::MEDIUM);
        assert_eq!(scale.title_medium_emphasized.weight, FontWeight::BOLD);
    }

    #[test]
    fn m3_label_small_emphasized_is_bold_weight() {
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.label_small.weight, FontWeight::MEDIUM);
        assert_eq!(scale.label_small_emphasized.weight, FontWeight::BOLD);
    }

    #[test]
    fn m3_body_large_emphasized_tracking_differs_from_base() {
        // body_large is the one role whose emphasized letter spacing also
        // differs from its own baseline (0.5px -> 0.15px), per the primary
        // source's BodyLargeEmphasizedTracking (see module docs).
        let scale = TypeScale::m3(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.body_large.letter_spacing, 0.5);
        assert_eq!(scale.body_large_emphasized.letter_spacing, 0.15);
    }

    #[test]
    fn cupertino_emphasized_slots_are_all_semibold() {
        // Cupertino emphasized forces Semibold across every slot — SF Pro's
        // mined text styles have no weight above Semibold to step up to
        // (see module docs' "Cupertino mapping" paragraph).
        let scale = TypeScale::cupertino(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large_emphasized.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.body_large_emphasized.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.label_small_emphasized.weight, FontWeight::SEMI_BOLD);
        // title_large/title_medium map from SF Pro Headline, already
        // Semibold by default — their emphasized sibling is therefore
        // numerically identical to base (a faithful mapping outcome, not a
        // bug — see module docs).
        assert_eq!(scale.title_large.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.title_large_emphasized.weight, FontWeight::SEMI_BOLD);
    }

    #[test]
    fn cupertino_emphasized_keeps_base_size_and_family() {
        let scale = TypeScale::cupertino(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.body_large.size, scale.body_large_emphasized.size);
        assert_eq!(scale.body_large.family, scale.body_large_emphasized.family);
        assert_eq!(
            scale.body_large.line_height,
            scale.body_large_emphasized.line_height
        );
    }
}
