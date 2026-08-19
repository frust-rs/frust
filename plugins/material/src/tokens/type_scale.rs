//! [`type_scale`]: the Material 3 (+Expressive) type scale — 15 baseline
//! roles plus 15 `_emphasized` siblings (30 [`TextStyle`] slots total),
//! resolving to the bundled Roboto Flex family.
//!
//! Source for the numeric progression: <https://m3.material.io/styles/typography/type-scale-tokens>
//! (verified 2026-07-17; sizes in sp, treated 1:1 as logical px; line heights
//! are absolute logical pixels, `LineHeight::Absolute`, not a font-size-relative
//! ratio — M3 publishes them as fixed px values per token, not a ratio).
//! Cross-verified against Jetpack Compose Material3's generated token file
//! (`androidx.compose.material3.tokens.TypographyTokens`/`TypeScaleTokens`,
//! `VERSION: v0_103`, retrieved/verified 2026-07-18) — the emphasized-sibling
//! weight-step-up rule below traces to that source (see "Emphasized type
//! scale").
//!
//! # M3E true-up (2026-08-19)
//!
//! Re-verified against `paadevelopments/material_3_expressive` v1.0.8
//! (pub.dev; `github.com/paadevelopments/material_3_expressive`),
//! `lib/foundations/m3e_typography.dart`'s `M3ETypeScale.baseline()` factory
//! (retrieved 2026-08-19). **Zero divergence**: every one of the 15 baseline
//! roles' `(size, line_height, letter_spacing, weight)` below matches the
//! Dart source's `style(...)` call for that role exactly —
//!
//! | Role | `m3e_typography.dart` line | Dart tuple `(size, height, tracking, weight)` |
//! |---|---|---|
//! | `display_large` | `m3e_typography.dart:51` | `(57, 64, -0.25, w400)` |
//! | `display_medium` | `m3e_typography.dart:52` | `(45, 52, 0, w400)` |
//! | `display_small` | `m3e_typography.dart:53` | `(36, 44, 0, w400)` |
//! | `headline_large` | `m3e_typography.dart:54` | `(32, 40, 0, w400)` |
//! | `headline_medium` | `m3e_typography.dart:55` | `(28, 36, 0, w400)` |
//! | `headline_small` | `m3e_typography.dart:56` | `(24, 32, 0, w400)` |
//! | `title_large` | `m3e_typography.dart:57` | `(22, 28, 0, w400)` |
//! | `title_medium` | `m3e_typography.dart:58` | `(16, 24, 0.15, w500)` |
//! | `title_small` | `m3e_typography.dart:59` | `(14, 20, 0.1, w500)` |
//! | `body_large` | `m3e_typography.dart:60` | `(16, 24, 0.5, w400)` |
//! | `body_medium` | `m3e_typography.dart:61` | `(14, 20, 0.25, w400)` |
//! | `body_small` | `m3e_typography.dart:62` | `(12, 16, 0.4, w400)` |
//! | `label_large` | `m3e_typography.dart:63` | `(14, 20, 0.1, w500)` |
//! | `label_medium` | `m3e_typography.dart:64` | `(12, 16, 0.5, w500)` |
//! | `label_small` | `m3e_typography.dart:65` | `(11, 16, 0.5, w500)` |
//!
//! (Dart's `height` field is a font-size-relative ratio, `TextStyle(fontSize:
//! size, height: height / size, ...)` per `m3e_typography.dart:38-45` — the
//! same absolute line-height-in-px value this module already stores, just
//! expressed as Flutter's unitless multiplier; not a divergence.) This also
//! reinforces, rather than contests, the existing `titleLarge` note below: the
//! M3E reference agrees with m3.material.io that it is Regular (400), not the
//! secondary source's claimed 500.
//!
//! **The emphasized siblings are NOT diffable against this source.**
//! `m3e_typography.dart` carries no per-role emphasized table at all — M3E's
//! "emphasized" concept (`M3ETypeVariations.emphasized`,
//! `m3e_typography.dart:261-265`) is a single variable-font axis preset
//! (`FontVariation('wght', 600)` + `FontVariation('GRAD', 50)`) applied
//! uniformly across the whole scale via `M3ETypeScale.apply(fontVariations:
//! ...)`, structurally different from this catalog's per-role Compose-derived
//! weight-step-up table (Regular→Medium, Medium→Bold — see "Emphasized type
//! scale" below). Since the source provides no per-role emphasized values to
//! diff against, and `GRAD` has no expressible seam in `frust-text` anyway
//! (see this task's variable-axes spike), the existing per-role table is kept
//! unchanged — no correction to record.
//!
//! # Emphasized type scale
//!
//! [`TypeScale`] additionally carries 15 `_emphasized` variants (one per
//! baseline role, 30 slots total). **Role-count resolution:** an earlier
//! belief that emphasized variants were "15 baseline + 15 emphasized (30
//! total), applied to Display/Headline/Title roles" was contested — the "30
//! total" count was right but the "Display/Headline/Title only" scope was
//! wrong. Verified directly against the primary source: Jetpack Compose
//! Material3's generated token file (see citation above) — Compose's
//! `Typography` class doc comments enumerate an `*Emphasized` property for
//! *all 15* baseline roles: `displayLarge` through `labelSmall`, not a
//! Display/Headline/Title-only subset. This module follows that: every one of
//! the 15 baseline roles gets an emphasized sibling.
//!
//! **M3 deltas** (from the same Compose source): size and line height are
//! unchanged between a role's baseline and emphasized token — only weight
//! (and, for a handful of roles, letter spacing) shift. Weight always steps
//! up one rung from the baseline token's own weight: Regular → Medium for
//! every Regular-weight baseline role (`display_*`, `headline_*`,
//! `title_large`, `body_*`), and Medium → Bold for every Medium-weight
//! baseline role (`title_medium`, `title_small`, `label_*`) — so every
//! emphasized style is guaranteed to differ from its base in weight. Letter
//! spacing mostly matches the baseline value already in this module's
//! tables; `body_large` is the one role whose emphasized tracking differs
//! from its own baseline (0.5px baseline → 0.15px emphasized, matching the
//! source's `BodyLargeEmphasizedTracking`).
//!
//! # Family: Roboto Flex
//!
//! [`apply_type`] overrides `family` to [`material_family`] on every slot,
//! regardless of what `base.family` a caller passes — the same pattern
//! `frust-cupertino`'s `apply_type`/`sf_family_for_size` uses for its
//! Text/Display optical-size switch (`plugins/cupertino/src/tokens.rs`'s
//! `apply_type`, whose own doc comment states the same "overridden per slot,
//! not inherited from `base`" rule) and `frust-shadcn`'s `sans_family`/
//! `mono_family` (`plugins/shadcn/src/tokens/theme.rs:118-133`) use for their
//! own bundled faces.
//!
//! **Evidence for the decision** (family named, not left unset):
//! - The M3E reference bundles Roboto Flex as its example app's font and sets
//!   it explicitly (`example/pubspec.yaml:28-30`; `README.md:225-235`,
//!   `paadevelopments/material_3_expressive` v1.0.8) — this catalog mirrors
//!   that as the design-system's own baseline choice, the same way
//!   `frust-glyph`/`frust-shadcn` name their own bundled faces directly on
//!   their type scales rather than leaving `family` unset
//!   (`plugins/glyph/src/tokens/scales.rs`'s `Face::family`;
//!   `plugins/shadcn/src/tokens/theme.rs`'s `sans_family`/`mono_family`).
//! - **A named-but-unregistered family degrades gracefully, it does not
//!   break rendering**: `frust-text`'s [`FontFamily::Named`] doc comment
//!   states "a name that doesn't resolve on the current platform falls back
//!   per parley's own fallback behavior — no error surface is exposed here"
//!   (`crates/frust-text/src/style.rs:44-51`, line 48 for the fallback
//!   sentence itself). [`FontFamily::stack_with_generic`] is the
//!   fallback-safe constructor this module uses (over a bare
//!   [`FontFamily::named`]) so the stack always ends in a generic slot,
//!   matching every other bundled-font catalog's own choice.
//! - **A registered family resolves via the font's own `name` table**, not a
//!   hand-picked string: `TextContext::register_fonts` reports back the
//!   family fontique's `Collection::family_name` resolved the registered
//!   face into (`crates/frust-text/src/context.rs:607-616`), and shadows a
//!   same-named system family once registered
//!   (`crates/frust-text/src/context.rs:557-561`). The Roboto Flex face this
//!   family name targets reports family name `"Roboto Flex"` in its own
//!   `name` table (`name` ID 1) — verified with `fontTools.ttLib` against the
//!   upstream Google-Fonts-sourced release
//!   `material_3_expressive`'s own copy bundles
//!   (`example/assets/fonts/RobotoFlex.ttf`, the same face
//!   `plugins/material/fonts/RobotoFlex.ttf` is sourced from), so
//!   [`ROBOTO_FLEX_FAMILY`]'s literal string is what a registered face
//!   actually resolves to, not a guess.
//!
//! **Degrade documented**: an app that never calls `frust_material::install()`
//! (or otherwise never drains `frust::register_app_fonts` with the bundled
//! Roboto Flex bytes) never registers the family, so every role in this type
//! scale falls back to the platform's generic sans-serif — no error, no
//! panic, just a different (but still legible) rendered face. `install()`'s
//! own font-registration wiring is this crate's `baseline()` concern, not
//! this module's.

use frust::TypeScale;
use frust::authoring::text::{FontFamily, FontWeight, GenericSlot, LineHeight, TextStyle};

/// The family name Roboto Flex's own `name` table reports (`name` ID 1) —
/// what a font stack must name for the bundled face to resolve. See this
/// module's "Family: Roboto Flex" doc section for the verification evidence.
pub const ROBOTO_FLEX_FAMILY: &str = "Roboto Flex";

/// The Material 3 Expressive type family: the bundled Roboto Flex face, then
/// the platform's generic sans — so text still renders (in the system sans)
/// on a host that never registered the face. See this module's "Family:
/// Roboto Flex" doc section.
fn material_family() -> FontFamily {
    FontFamily::stack_with_generic([ROBOTO_FLEX_FAMILY], GenericSlot::SansSerif)
}

/// One type-scale token's numeric shape: `(size_px, line_height_px,
/// letter_spacing_px, weight)`.
type TypeToken = (f32, f32, f32, FontWeight);

const DISPLAY_LARGE: TypeToken = (57.0, 64.0, -0.25, FontWeight::REGULAR);
const DISPLAY_MEDIUM: TypeToken = (45.0, 52.0, 0.0, FontWeight::REGULAR);
const DISPLAY_SMALL: TypeToken = (36.0, 44.0, 0.0, FontWeight::REGULAR);
const HEADLINE_LARGE: TypeToken = (32.0, 40.0, 0.0, FontWeight::REGULAR);
const HEADLINE_MEDIUM: TypeToken = (28.0, 36.0, 0.0, FontWeight::REGULAR);
const HEADLINE_SMALL: TypeToken = (24.0, 32.0, 0.0, FontWeight::REGULAR);
const TITLE_LARGE: TypeToken = (22.0, 28.0, 0.0, FontWeight::REGULAR);
const TITLE_MEDIUM: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const TITLE_SMALL: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const BODY_LARGE: TypeToken = (16.0, 24.0, 0.5, FontWeight::REGULAR);
const BODY_MEDIUM: TypeToken = (14.0, 20.0, 0.25, FontWeight::REGULAR);
const BODY_SMALL: TypeToken = (12.0, 16.0, 0.4, FontWeight::REGULAR);
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const LABEL_MEDIUM: TypeToken = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_SMALL: TypeToken = (11.0, 16.0, 0.5, FontWeight::MEDIUM);

// M3-Expressive emphasized tokens: same size/line-height as the matching
// baseline `TypeToken` above in every case; weight steps up one rung from
// the baseline role's own weight (Regular -> Medium, Medium -> Bold) and
// letter spacing is the source's `*Emphasized*Tracking` value (see this
// module's doc comment for the primary-source citation and resolution of
// the contested Display/Headline/Title-only scope claim).
const DISPLAY_LARGE_EMPHASIZED: TypeToken = (57.0, 64.0, 0.0, FontWeight::MEDIUM);
const DISPLAY_MEDIUM_EMPHASIZED: TypeToken = (45.0, 52.0, 0.0, FontWeight::MEDIUM);
const DISPLAY_SMALL_EMPHASIZED: TypeToken = (36.0, 44.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_LARGE_EMPHASIZED: TypeToken = (32.0, 40.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_MEDIUM_EMPHASIZED: TypeToken = (28.0, 36.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_SMALL_EMPHASIZED: TypeToken = (24.0, 32.0, 0.0, FontWeight::MEDIUM);
const TITLE_LARGE_EMPHASIZED: TypeToken = (22.0, 28.0, 0.0, FontWeight::MEDIUM);
const TITLE_MEDIUM_EMPHASIZED: TypeToken = (16.0, 24.0, 0.15, FontWeight::BOLD);
const TITLE_SMALL_EMPHASIZED: TypeToken = (14.0, 20.0, 0.1, FontWeight::BOLD);
const BODY_LARGE_EMPHASIZED: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const BODY_MEDIUM_EMPHASIZED: TypeToken = (14.0, 20.0, 0.25, FontWeight::MEDIUM);
const BODY_SMALL_EMPHASIZED: TypeToken = (12.0, 16.0, 0.4, FontWeight::MEDIUM);
const LABEL_LARGE_EMPHASIZED: TypeToken = (14.0, 20.0, 0.1, FontWeight::BOLD);
const LABEL_MEDIUM_EMPHASIZED: TypeToken = (12.0, 16.0, 0.5, FontWeight::BOLD);
const LABEL_SMALL_EMPHASIZED: TypeToken = (11.0, 16.0, 0.5, FontWeight::BOLD);

/// Applies one `token` onto `base`: `family` is always [`material_family`]
/// (overridden regardless of `base`, see this module's "Family: Roboto Flex"
/// doc section); `style`/`color`/`align` are preserved from `base`.
fn apply_type(base: &TextStyle, token: TypeToken) -> TextStyle {
    let (size, line_height_px, letter_spacing, weight) = token;
    TextStyle {
        family: material_family(),
        size,
        weight,
        letter_spacing,
        line_height: LineHeight::Absolute(line_height_px),
        ..base.clone()
    }
}

/// Builds the Material 3 baseline type scale from `base` (its
/// `style`/`color`/`align` are preserved on every token; `size`/`weight`/
/// `letter_spacing`/`line_height`/`family` are all Material-specified —
/// `family` is overridden to the bundled Roboto Flex stack regardless of
/// `base`, see this module's "Family: Roboto Flex" doc section for why).
pub fn type_scale(base: &TextStyle) -> TypeScale {
    TypeScale {
        display_large: apply_type(base, DISPLAY_LARGE),
        display_medium: apply_type(base, DISPLAY_MEDIUM),
        display_small: apply_type(base, DISPLAY_SMALL),
        headline_large: apply_type(base, HEADLINE_LARGE),
        headline_medium: apply_type(base, HEADLINE_MEDIUM),
        headline_small: apply_type(base, HEADLINE_SMALL),
        title_large: apply_type(base, TITLE_LARGE),
        title_medium: apply_type(base, TITLE_MEDIUM),
        title_small: apply_type(base, TITLE_SMALL),
        body_large: apply_type(base, BODY_LARGE),
        body_medium: apply_type(base, BODY_MEDIUM),
        body_small: apply_type(base, BODY_SMALL),
        label_large: apply_type(base, LABEL_LARGE),
        label_medium: apply_type(base, LABEL_MEDIUM),
        label_small: apply_type(base, LABEL_SMALL),
        display_large_emphasized: apply_type(base, DISPLAY_LARGE_EMPHASIZED),
        display_medium_emphasized: apply_type(base, DISPLAY_MEDIUM_EMPHASIZED),
        display_small_emphasized: apply_type(base, DISPLAY_SMALL_EMPHASIZED),
        headline_large_emphasized: apply_type(base, HEADLINE_LARGE_EMPHASIZED),
        headline_medium_emphasized: apply_type(base, HEADLINE_MEDIUM_EMPHASIZED),
        headline_small_emphasized: apply_type(base, HEADLINE_SMALL_EMPHASIZED),
        title_large_emphasized: apply_type(base, TITLE_LARGE_EMPHASIZED),
        title_medium_emphasized: apply_type(base, TITLE_MEDIUM_EMPHASIZED),
        title_small_emphasized: apply_type(base, TITLE_SMALL_EMPHASIZED),
        body_large_emphasized: apply_type(base, BODY_LARGE_EMPHASIZED),
        body_medium_emphasized: apply_type(base, BODY_MEDIUM_EMPHASIZED),
        body_small_emphasized: apply_type(base, BODY_SMALL_EMPHASIZED),
        label_large_emphasized: apply_type(base, LABEL_LARGE_EMPHASIZED),
        label_medium_emphasized: apply_type(base, LABEL_MEDIUM_EMPHASIZED),
        label_small_emphasized: apply_type(base, LABEL_SMALL_EMPHASIZED),
    }
}

#[cfg(test)]
mod tests {
    use peniko::Color;

    use super::*;

    #[test]
    fn type_scale_display_large_matches_table() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large.size, 57.0);
        assert_eq!(scale.display_large.line_height, LineHeight::Absolute(64.0));
        assert_eq!(scale.display_large.letter_spacing, -0.25);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn type_scale_preserves_base_style_and_color_but_overrides_family() {
        let base = TextStyle {
            family: FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = type_scale(&base);
        assert_ne!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.family, material_family());
        assert_eq!(scale.body_large.color, base.color);
        assert_eq!(scale.body_large.style, base.style);
    }

    #[test]
    fn type_scale_every_role_resolves_to_roboto_flex() {
        let scale = type_scale(&TextStyle::default());
        let expected = material_family();
        let roles: [&TextStyle; 30] = [
            &scale.display_large,
            &scale.display_medium,
            &scale.display_small,
            &scale.headline_large,
            &scale.headline_medium,
            &scale.headline_small,
            &scale.title_large,
            &scale.title_medium,
            &scale.title_small,
            &scale.body_large,
            &scale.body_medium,
            &scale.body_small,
            &scale.label_large,
            &scale.label_medium,
            &scale.label_small,
            &scale.display_large_emphasized,
            &scale.display_medium_emphasized,
            &scale.display_small_emphasized,
            &scale.headline_large_emphasized,
            &scale.headline_medium_emphasized,
            &scale.headline_small_emphasized,
            &scale.title_large_emphasized,
            &scale.title_medium_emphasized,
            &scale.title_small_emphasized,
            &scale.body_large_emphasized,
            &scale.body_medium_emphasized,
            &scale.body_small_emphasized,
            &scale.label_large_emphasized,
            &scale.label_medium_emphasized,
            &scale.label_small_emphasized,
        ];
        for role in roles {
            assert_eq!(role.family, expected);
        }
    }

    #[test]
    fn type_scale_every_emphasized_role_differs_in_weight_from_its_base() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
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
        for (b, emphasized) in pairs {
            assert_ne!(
                b.weight, emphasized.weight,
                "expected emphasized weight to differ from base weight"
            );
        }
    }

    /// Per-role metric table: every one of the 30 slots pinned against its
    /// literal `(size, line_height_px, letter_spacing, weight)` — the
    /// `TypeToken` tuples above, transcribed as literals so this test catches
    /// an accidental edit to those consts the same way it would a source
    /// transcription error. Values match `m3e_typography.dart:51-65` for the
    /// 15 baseline roles (see this module's "M3E true-up" doc section); the
    /// emphasized 15 are this catalog's own Compose-derived weight-step-up
    /// table (see "Emphasized type scale").
    #[test]
    fn type_scale_every_role_matches_its_literal_metric_table() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        let table: [(&str, &TextStyle, f32, f32, f32, FontWeight); 30] = [
            (
                "display_large",
                &scale.display_large,
                57.0,
                64.0,
                -0.25,
                FontWeight::REGULAR,
            ),
            (
                "display_medium",
                &scale.display_medium,
                45.0,
                52.0,
                0.0,
                FontWeight::REGULAR,
            ),
            (
                "display_small",
                &scale.display_small,
                36.0,
                44.0,
                0.0,
                FontWeight::REGULAR,
            ),
            (
                "headline_large",
                &scale.headline_large,
                32.0,
                40.0,
                0.0,
                FontWeight::REGULAR,
            ),
            (
                "headline_medium",
                &scale.headline_medium,
                28.0,
                36.0,
                0.0,
                FontWeight::REGULAR,
            ),
            (
                "headline_small",
                &scale.headline_small,
                24.0,
                32.0,
                0.0,
                FontWeight::REGULAR,
            ),
            (
                "title_large",
                &scale.title_large,
                22.0,
                28.0,
                0.0,
                FontWeight::REGULAR,
            ),
            (
                "title_medium",
                &scale.title_medium,
                16.0,
                24.0,
                0.15,
                FontWeight::MEDIUM,
            ),
            (
                "title_small",
                &scale.title_small,
                14.0,
                20.0,
                0.1,
                FontWeight::MEDIUM,
            ),
            (
                "body_large",
                &scale.body_large,
                16.0,
                24.0,
                0.5,
                FontWeight::REGULAR,
            ),
            (
                "body_medium",
                &scale.body_medium,
                14.0,
                20.0,
                0.25,
                FontWeight::REGULAR,
            ),
            (
                "body_small",
                &scale.body_small,
                12.0,
                16.0,
                0.4,
                FontWeight::REGULAR,
            ),
            (
                "label_large",
                &scale.label_large,
                14.0,
                20.0,
                0.1,
                FontWeight::MEDIUM,
            ),
            (
                "label_medium",
                &scale.label_medium,
                12.0,
                16.0,
                0.5,
                FontWeight::MEDIUM,
            ),
            (
                "label_small",
                &scale.label_small,
                11.0,
                16.0,
                0.5,
                FontWeight::MEDIUM,
            ),
            (
                "display_large_emphasized",
                &scale.display_large_emphasized,
                57.0,
                64.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "display_medium_emphasized",
                &scale.display_medium_emphasized,
                45.0,
                52.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "display_small_emphasized",
                &scale.display_small_emphasized,
                36.0,
                44.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "headline_large_emphasized",
                &scale.headline_large_emphasized,
                32.0,
                40.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "headline_medium_emphasized",
                &scale.headline_medium_emphasized,
                28.0,
                36.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "headline_small_emphasized",
                &scale.headline_small_emphasized,
                24.0,
                32.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "title_large_emphasized",
                &scale.title_large_emphasized,
                22.0,
                28.0,
                0.0,
                FontWeight::MEDIUM,
            ),
            (
                "title_medium_emphasized",
                &scale.title_medium_emphasized,
                16.0,
                24.0,
                0.15,
                FontWeight::BOLD,
            ),
            (
                "title_small_emphasized",
                &scale.title_small_emphasized,
                14.0,
                20.0,
                0.1,
                FontWeight::BOLD,
            ),
            (
                "body_large_emphasized",
                &scale.body_large_emphasized,
                16.0,
                24.0,
                0.15,
                FontWeight::MEDIUM,
            ),
            (
                "body_medium_emphasized",
                &scale.body_medium_emphasized,
                14.0,
                20.0,
                0.25,
                FontWeight::MEDIUM,
            ),
            (
                "body_small_emphasized",
                &scale.body_small_emphasized,
                12.0,
                16.0,
                0.4,
                FontWeight::MEDIUM,
            ),
            (
                "label_large_emphasized",
                &scale.label_large_emphasized,
                14.0,
                20.0,
                0.1,
                FontWeight::BOLD,
            ),
            (
                "label_medium_emphasized",
                &scale.label_medium_emphasized,
                12.0,
                16.0,
                0.5,
                FontWeight::BOLD,
            ),
            (
                "label_small_emphasized",
                &scale.label_small_emphasized,
                11.0,
                16.0,
                0.5,
                FontWeight::BOLD,
            ),
        ];
        for (name, style, size, line_height_px, letter_spacing, weight) in table {
            assert_eq!(style.size, size, "{name} size");
            assert_eq!(
                style.line_height,
                LineHeight::Absolute(line_height_px),
                "{name} line_height"
            );
            assert_eq!(
                style.letter_spacing, letter_spacing,
                "{name} letter_spacing"
            );
            assert_eq!(style.weight, weight, "{name} weight");
        }
    }
}
