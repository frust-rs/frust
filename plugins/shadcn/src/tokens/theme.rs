//! The assembled shadcn [`Theme`]: the vendored token tables folded onto
//! frust's fixed token scales.
//!
//! [`theme()`](fn@theme) is the entry point ([`install`](crate::install) seeds it);
//! [`theme_for`] builds any of the seven base presets, and the six
//! `theme_*` constructors below name them by hand for call-site convenience.
//!
//! # How a theme is composed
//!
//! Through [`ThemeBuilder`] over [`Theme::neutral`], the way a third-party
//! design system is meant to compose one (the `sample-design` precedent), never
//! by filling a `Theme` literal field by field:
//!
//! 1. baseline: [`Theme::neutral`] — the design-language-free floor, so every
//!    token this module does not set is a deliberate "no opinion" rather than an
//!    inherited Material one.
//! 2. `colors_light`/`colors_dark`: [`color_scheme`]'s 46-role mapping.
//! 3. `type_scale`: [`type_scale`] — the bundled families over the neutral
//!    numbers.
//! 4. `shape`: [`shape_scale`] — shadcn's derived radius ladder.
//! 5. `design_language`: [`DesignLanguage::Custom`]`(`[`SHADCN_DESIGN_LANGUAGE`]`)`.
//! 6. `extension`: [`ShadcnTokens`] (ring/chart/sidebar/radius) and
//!    [`NativeTypefaces`] (the bundled faces, for native controls).
//!
//! `Elevation`, `MotionScheme`, and `GlassScale` stay at the neutral floor:
//! shadcn authors no motion or glass vocabulary at all, and its shadows are
//! Tailwind's flat CSS ladder rather than a per-level elevation table — that
//! ladder lives in [`crate::style`] as constants a component paints with
//! directly, which is where a CSS-derived shadow belongs.
//!
//! **Brightness is deliberately not pinned.** The builder leaves
//! `Theme::neutral`'s own value in place; a shell re-derives light/dark from the
//! platform against whatever base [`install`](crate::install) seeded, and
//! pinning it here would fight that (the same rationale `sample-design`'s
//! `sample_theme` records).
//!
//! # The 46-role mapping, and what it had to decide
//!
//! shadcn authors ~18 color roles; `ColorScheme` has 46. [`color_scheme`] holds
//! the whole mapping, each judgment call commented at the field it decides. The
//! ones worth knowing before reading it:
//!
//! - **`primary` is the solid fill, not the ink.** A shadcn default button is
//!   `bg-primary text-primary-foreground`, so `primary`/`on_primary` map
//!   straight across. shadcn has no *container* variant of the accent, so
//!   `primary_container`/`on_primary_container` take `accent`/`accent-foreground`
//!   — the wash shadcn actually paints for a hover/selected state, which in this
//!   monochrome base palette *is* the soft counterpart of `primary`.
//! - **`secondary`/`muted`/`accent` carry the same value upstream** in every
//!   shipped preset (`oklch(0.97 0 0)` light, `0.269` dark). They still map to
//!   distinct frust roles (`secondary`, `surface_container_highest`, `tertiary`)
//!   rather than being collapsed, so a preset that ever splits them keeps
//!   working — the duplication is upstream's, not this port's.
//! - **`destructive` has no paired foreground token.** shadcn paints
//!   `text-white` over it (`button.tsx`'s `destructive` variant), so
//!   `on_error` is white rather than a token read. `error_container` is
//!   `destructive` at 10% alpha — shadcn's own `bg-destructive/10` — kept as
//!   alpha rather than flattened, so it composites over whatever surface it
//!   lands on.
//! - **`--ring` gets no `ColorScheme` role.** It rides [`ShadcnTokens`]; see
//!   that module's docs for why `surface_tint` was the wrong home. `surface_tint`
//!   keeps the M3 `== primary` convention every other theme in the tree keeps.
//! - **The 12 `*_fixed*` roles are filled from the light table in both
//!   schemes.** That is what "fixed" means in M3 (brightness-invariant), and it
//!   keeps a widget that paints one from inheriting the neutral floor's
//!   slate-blue accent, which would be the only non-shadcn hue in the theme.
//!   shadcn has no dim variant, so `*_fixed_dim` repeats `*_fixed`.
//! - **`surface_dim`/`surface_bright` swap by brightness.** In light mode the
//!   dimmest surface is `muted` and the brightest is `background`; in dark mode
//!   it is the other way round. Assigning one order to both brightnesses would
//!   make "dim" mean "lighter than the surface" in one of them.
//! - **`inverse_primary` is the *other* brightness's `primary`** — an inverse
//!   surface is the other brightness's surface, so the accent that reads on it
//!   is the other brightness's accent.

use frust::{
    Brightness, Color, ColorScheme, DesignLanguage, FontFace, NativeTypefaces, ShapeScale, Theme,
    ThemeBuilder, TypeScale,
    authoring::text::{FontFamily, GenericSlot, TextStyle},
};

use super::extension::ShadcnTokens;
use super::fonts::{self, INTER_VARIABLE_INDEX};
use super::palette::{ShadcnBase, ShadcnPalette};

/// This design system's stable identity tag, carried on
/// [`Theme::design_language`] as [`DesignLanguage::Custom`].
///
/// A host or widget that branches on design language sees this id rather than
/// mistaking the system for Material; every built-in `==` branch site treats an
/// unrecognized `Custom` tag as the neutral/System path, which is what a
/// web-derived system with no platform-native counterpart wants.
pub const SHADCN_DESIGN_LANGUAGE: &str = "shadcn";

/// The family name Inter's own `name` table reports (`name` ID 1) — what a
/// font stack must name for the bundled face to resolve. Note it is `"Inter
/// Variable"`, not `"Inter"`: the variable release names itself that.
pub const INTER_FAMILY: &str = "Inter Variable";

/// The family name JetBrains Mono's own `name` table reports (`name` ID 1).
pub const JETBRAINS_MONO_FAMILY: &str = "JetBrains Mono";

/// Alpha of the `destructive` wash `error_container` carries — shadcn's
/// `bg-destructive/10`.
const DESTRUCTIVE_WASH_ALPHA: f32 = 0.10;

/// Alpha of the modal scrim — shadcn's `bg-black/50`
/// (`dialog`/`alert-dialog`/`sheet`/`drawer` overlays all use it verbatim).
const SCRIM_ALPHA: f32 = 0.50;

/// Return `color` with its alpha channel replaced by `alpha` (the same
/// per-module helper shape `frust-widgets` and the other catalogs carry).
const fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The sans font stack: the bundled Inter face, then the platform's generic
/// sans — so text still renders (in the system sans) if a host never drained
/// the font registry.
pub fn sans_family() -> FontFamily {
    FontFamily::stack_with_generic([INTER_FAMILY], GenericSlot::SansSerif)
}

/// The monospace font stack: the bundled JetBrains Mono face, then the
/// platform's generic monospace.
///
/// `TypeScale` has no monospace slot, so this is the seam a component that needs
/// mono text (`kbd`, a code block, tabular figures) builds its own `TextStyle`
/// from — the same way it would name `font-mono` in a shadcn class list.
pub fn mono_family() -> FontFamily {
    FontFamily::stack_with_generic([JETBRAINS_MONO_FAMILY], GenericSlot::Monospace)
}

/// The shadcn type scale: [`TypeScale::neutral`]'s numeric scale with every
/// slot's family swapped to [`sans_family`].
///
/// **Family-only.** shadcn sizes type per component from Tailwind's `text-xs`/
/// `text-sm`/`text-base` steps rather than from a document-wide scale, so those
/// sizes live in [`crate::style`] where a component reads them, and this scale
/// exists to do one job: make the bundled Inter face the theme's actual text
/// family, for baseline widgets and shadcn components alike. Re-deriving the 30
/// M3 slots into a shadcn vocabulary they have no counterpart for would invent
/// a scale upstream does not author.
pub fn type_scale(base: &TextStyle) -> TypeScale {
    let scale = TypeScale::neutral(base);
    let sans = |style: &TextStyle| TextStyle {
        family: sans_family(),
        ..style.clone()
    };
    TypeScale {
        display_large: sans(&scale.display_large),
        display_medium: sans(&scale.display_medium),
        display_small: sans(&scale.display_small),
        headline_large: sans(&scale.headline_large),
        headline_medium: sans(&scale.headline_medium),
        headline_small: sans(&scale.headline_small),
        title_large: sans(&scale.title_large),
        title_medium: sans(&scale.title_medium),
        title_small: sans(&scale.title_small),
        body_large: sans(&scale.body_large),
        body_medium: sans(&scale.body_medium),
        body_small: sans(&scale.body_small),
        label_large: sans(&scale.label_large),
        label_medium: sans(&scale.label_medium),
        label_small: sans(&scale.label_small),
        display_large_emphasized: sans(&scale.display_large_emphasized),
        display_medium_emphasized: sans(&scale.display_medium_emphasized),
        display_small_emphasized: sans(&scale.display_small_emphasized),
        headline_large_emphasized: sans(&scale.headline_large_emphasized),
        headline_medium_emphasized: sans(&scale.headline_medium_emphasized),
        headline_small_emphasized: sans(&scale.headline_small_emphasized),
        title_large_emphasized: sans(&scale.title_large_emphasized),
        title_medium_emphasized: sans(&scale.title_medium_emphasized),
        title_small_emphasized: sans(&scale.title_small_emphasized),
        body_large_emphasized: sans(&scale.body_large_emphasized),
        body_medium_emphasized: sans(&scale.body_medium_emphasized),
        body_small_emphasized: sans(&scale.body_small_emphasized),
        label_large_emphasized: sans(&scale.label_large_emphasized),
        label_medium_emphasized: sans(&scale.label_medium_emphasized),
        label_small_emphasized: sans(&scale.label_small_emphasized),
    }
}

/// The shadcn shape scale: the seven derived radii
/// ([`ShadcnRadius`](super::extension::ShadcnRadius)) spread across
/// `ShapeScale`'s ten slots.
///
/// shadcn has seven steps and `ShapeScale` has nine finite ones, so the top slot
/// clamps to `4xl` (the Glyph precedent for a shorter source scale) rather than
/// extrapolating an eighth step upstream never authors. `full` stays the pill
/// sentinel every consumer resolves against a box.
pub const fn shape_scale() -> ShapeScale {
    let r = super::extension::ShadcnRadius::shadcn();
    ShapeScale {
        none: 0.0,
        extra_small: r.sm,
        small: r.md,
        medium: r.lg,
        large: r.xl,
        large_increased: r.xl2,
        extra_large: r.xl3,
        extra_large_increased: r.xl4,
        extra_extra_large: r.xl4, // clamped: shadcn's ladder stops at 4xl
        full: f64::INFINITY,
    }
}

/// Fold a base preset's two token tables onto frust's 46-role `ColorScheme` for
/// one `brightness`.
///
/// Both tables are taken because three role groups need the one this brightness
/// is *not*: the 12 `*_fixed*` roles (always the light table, since "fixed"
/// means brightness-invariant) and `inverse_primary` (the other brightness's
/// accent). See the [module docs](self) for the reasoning behind each judgment
/// call; the per-field comments below name which decision each line is.
pub fn color_scheme(
    light: &ShadcnPalette,
    dark: &ShadcnPalette,
    brightness: Brightness,
) -> ColorScheme {
    let (p, other) = match brightness {
        Brightness::Light => (light, dark),
        Brightness::Dark => (dark, light),
    };
    // The dimmest/brightest surface swap ends: light mode dims toward `muted`,
    // dark mode brightens toward it.
    let (dim, bright) = match brightness {
        Brightness::Light => (p.muted, p.background),
        Brightness::Dark => (p.background, p.muted),
    };

    ColorScheme {
        // The accent: shadcn's `primary` is the solid fill, its `accent` the
        // soft wash that stands in for M3's container pair.
        primary: p.primary,
        on_primary: p.primary_foreground,
        primary_container: p.accent,
        on_primary_container: p.accent_foreground,
        primary_fixed: light.primary,
        primary_fixed_dim: light.primary, // no dim variant upstream
        on_primary_fixed: light.primary_foreground,
        on_primary_fixed_variant: light.muted_foreground,

        // `secondary` maps straight across; its container repeats it, since
        // shadcn draws no distinction between a secondary fill and a secondary
        // container.
        secondary: p.secondary,
        on_secondary: p.secondary_foreground,
        secondary_container: p.secondary,
        on_secondary_container: p.secondary_foreground,
        secondary_fixed: light.secondary,
        secondary_fixed_dim: light.secondary,
        on_secondary_fixed: light.secondary_foreground,
        on_secondary_fixed_variant: light.muted_foreground,

        // shadcn has no third hue, so `tertiary` takes the `accent` wash — the
        // nearest thing to a third role it authors.
        tertiary: p.accent,
        on_tertiary: p.accent_foreground,
        tertiary_container: p.accent,
        on_tertiary_container: p.accent_foreground,
        tertiary_fixed: light.accent,
        tertiary_fixed_dim: light.accent,
        on_tertiary_fixed: light.accent_foreground,
        on_tertiary_fixed_variant: light.muted_foreground,

        // `destructive` → the error family. White ink is shadcn's own choice
        // (`bg-destructive text-white`), not a token read; the container is its
        // 10% wash, kept translucent.
        error: p.destructive,
        on_error: Color::WHITE,
        error_container: with_alpha(p.destructive, DESTRUCTIVE_WASH_ALPHA),
        on_error_container: p.destructive,

        // The surface ladder: background → card/popover → muted, which is
        // increasing emphasis in both brightnesses (light darkens, dark
        // lightens).
        surface: p.background,
        on_surface: p.foreground,
        on_surface_variant: p.muted_foreground,
        surface_dim: dim,
        surface_bright: bright,
        surface_container_lowest: p.background,
        surface_container_low: p.card,
        surface_container: p.card,
        surface_container_high: p.popover,
        surface_container_highest: p.muted,

        outline: p.border,
        outline_variant: p.input,
        // Tailwind's shadows are `rgb(0 0 0 / α)`; the alpha travels with each
        // shadow constant in `crate::style`, so the role itself is plain black.
        shadow: Color::BLACK,
        scrim: with_alpha(Color::BLACK, SCRIM_ALPHA),
        // An inverse surface is the other brightness's surface — and the accent
        // that reads on it is that brightness's accent.
        inverse_surface: p.foreground,
        inverse_on_surface: p.background,
        inverse_primary: other.primary,
        // Keeps the M3 `surface_tint == primary` convention; the focus ring
        // lives in `ShadcnTokens`, not here.
        surface_tint: p.primary,
    }
}

/// The `neutral` preset's light-mode `ColorScheme` — the flat-named constructor
/// the external design-system contract asks for.
pub fn color_scheme_light() -> ColorScheme {
    let base = ShadcnBase::Neutral;
    color_scheme(&base.light(), &base.dark(), Brightness::Light)
}

/// The `neutral` preset's dark-mode `ColorScheme`.
pub fn color_scheme_dark() -> ColorScheme {
    let base = ShadcnBase::Neutral;
    color_scheme(&base.light(), &base.dark(), Brightness::Dark)
}

/// The native-control typeface binding every `theme*()` attaches: the bundled
/// Inter face in both slots.
///
/// shadcn names one sans family for buttons and body text alike, so unlike
/// Glyph's display/body split both slots carry the same face. Both are taken
/// **out of [`fonts::font_data`]'s own array** rather than re-referenced from the
/// underlying constants: a native host de-duplicates published payloads by byte
/// identity (address + length), so a slot's face and the bytes a shell registers
/// through [`install`](crate::install) must be the *same* `&'static [u8]`, not
/// merely equal ones.
pub fn native_typefaces() -> NativeTypefaces {
    match fonts::font_data().get(INTER_VARIABLE_INDEX).copied() {
        Some(bytes) => NativeTypefaces::uniform(FontFace::new(INTER_FAMILY, bytes)),
        None => NativeTypefaces::default(),
    }
}

/// The shadcn theme, built from any base preset. See the [module docs](self) for
/// the composition order and the mapping decisions.
pub fn theme_for(base: ShadcnBase) -> Theme {
    let (light, dark) = (base.light(), base.dark());
    ThemeBuilder::new(Theme::neutral())
        .colors_light(color_scheme(&light, &dark, Brightness::Light))
        .colors_dark(color_scheme(&light, &dark, Brightness::Dark))
        .type_scale(type_scale(&TextStyle::default()))
        .shape(shape_scale())
        .design_language(DesignLanguage::Custom(SHADCN_DESIGN_LANGUAGE))
        .extension(ShadcnTokens::for_base(base))
        .extension(native_typefaces())
        .build()
}

/// The shadcn theme: the `neutral` base preset, shadcn's own default.
///
/// This is what [`install`](crate::install) seeds as the app's default theme.
pub fn theme() -> Theme {
    theme_for(ShadcnBase::Neutral)
}

/// The shadcn theme on the `stone` base preset (warm grey).
pub fn theme_stone() -> Theme {
    theme_for(ShadcnBase::Stone)
}

/// The shadcn theme on the `zinc` base preset (cool grey).
pub fn theme_zinc() -> Theme {
    theme_for(ShadcnBase::Zinc)
}

/// The shadcn theme on the `mauve` base preset (pink-leaning grey).
pub fn theme_mauve() -> Theme {
    theme_for(ShadcnBase::Mauve)
}

/// The shadcn theme on the `olive` base preset (green-leaning grey).
pub fn theme_olive() -> Theme {
    theme_for(ShadcnBase::Olive)
}

/// The shadcn theme on the `mist` base preset (blue-leaning grey).
pub fn theme_mist() -> Theme {
    theme_for(ShadcnBase::Mist)
}

/// The shadcn theme on the `taupe` base preset (brown-leaning grey).
pub fn theme_taupe() -> Theme {
    theme_for(ShadcnBase::Taupe)
}

#[cfg(test)]
mod tests {
    use super::super::extension::{RADIUS_BASE, ShadcnRadius};
    #[cfg(feature = "bundled-fonts")]
    use super::super::fonts::JETBRAINS_MONO_VARIABLE_INDEX;
    use super::*;
    use frust::authoring::text::FamilyName;

    #[test]
    fn theme_carries_the_custom_design_language_tag() {
        let t = theme();
        assert_eq!(
            t.design_language,
            DesignLanguage::Custom(SHADCN_DESIGN_LANGUAGE)
        );
        // Never mistakeable for a built-in language.
        assert_ne!(t.design_language, DesignLanguage::Material3);
        assert_ne!(t.design_language, DesignLanguage::Cupertino);
        assert_ne!(t.design_language, DesignLanguage::Glyph);
    }

    #[test]
    fn theme_is_internally_consistent_and_round_trips_its_parts() {
        let t = theme();
        assert_eq!(t.light, color_scheme_light());
        assert_eq!(t.dark, color_scheme_dark());
        assert_eq!(t.shape, shape_scale());
        assert_eq!(t.type_scale, type_scale(&TextStyle::default()));
        assert_eq!(t.extension::<ShadcnTokens>(), Some(&ShadcnTokens::shadcn()));
        assert_eq!(t.extension::<NativeTypefaces>(), Some(&native_typefaces()));
        // Survives the clone the framework's two delivery paths take.
        let c = t.clone();
        assert_eq!(c.extension::<ShadcnTokens>(), Some(&ShadcnTokens::shadcn()));
        assert_eq!(c.light, t.light);
    }

    #[test]
    fn theme_does_not_pin_brightness() {
        // A shell re-derives light/dark from the platform against the seeded
        // base; the builder must leave the neutral floor's own value alone.
        assert_eq!(theme().brightness, Theme::neutral().brightness);
        let dark = theme().with_brightness(Brightness::Dark);
        assert_eq!(dark.scheme(), &dark.dark);
        assert_eq!(
            dark.design_language,
            DesignLanguage::Custom(SHADCN_DESIGN_LANGUAGE)
        );
    }

    #[test]
    fn light_scheme_spot_roles_map_the_vendored_tokens() {
        let p = ShadcnBase::Neutral.light();
        let s = color_scheme_light();
        assert_eq!(s.surface, p.background);
        assert_eq!(s.on_surface, p.foreground);
        assert_eq!(s.on_surface_variant, p.muted_foreground);
        assert_eq!(s.primary, p.primary);
        assert_eq!(s.on_primary, p.primary_foreground);
        assert_eq!(s.secondary, p.secondary);
        assert_eq!(s.tertiary, p.accent);
        assert_eq!(s.error, p.destructive);
        assert_eq!(s.on_error, Color::WHITE);
        assert_eq!(s.outline, p.border);
        assert_eq!(s.outline_variant, p.input);
        assert_eq!(s.surface_container, p.card);
        assert_eq!(s.surface_container_high, p.popover);
        assert_eq!(s.surface_container_highest, p.muted);
        assert_eq!(s.surface_tint, p.primary);
        // Concrete values, so a silent table edit is caught too.
        assert_eq!(s.surface, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(s.on_surface, Color::from_rgb8(0x0A, 0x0A, 0x0A));
    }

    #[test]
    fn dark_scheme_spot_roles_map_the_vendored_tokens() {
        let p = ShadcnBase::Neutral.dark();
        let s = color_scheme_dark();
        assert_eq!(s.surface, p.background);
        assert_eq!(s.on_surface, p.foreground);
        assert_eq!(s.primary, p.primary);
        assert_eq!(s.error, p.destructive);
        assert_eq!(s.surface_container, p.card);
        assert_eq!(s.surface, Color::from_rgb8(0x0A, 0x0A, 0x0A));
        // The alpha border survives the fold onto `outline`.
        assert_eq!(s.outline, p.border);
        assert!(s.outline.components[3] < 1.0);
    }

    #[test]
    fn surface_dim_and_bright_swap_with_brightness() {
        let light = color_scheme_light();
        let dark = color_scheme_dark();
        assert_eq!(light.surface_dim, ShadcnBase::Neutral.light().muted);
        assert_eq!(light.surface_bright, ShadcnBase::Neutral.light().background);
        assert_eq!(dark.surface_dim, ShadcnBase::Neutral.dark().background);
        assert_eq!(dark.surface_bright, ShadcnBase::Neutral.dark().muted);
    }

    #[test]
    fn fixed_roles_are_brightness_invariant_and_inverse_primary_crosses_over() {
        let light = color_scheme_light();
        let dark = color_scheme_dark();
        assert_eq!(light.primary_fixed, dark.primary_fixed);
        assert_eq!(light.secondary_fixed, dark.secondary_fixed);
        assert_eq!(light.tertiary_fixed, dark.tertiary_fixed);
        assert_eq!(light.primary_fixed, ShadcnBase::Neutral.light().primary);
        // ...and none of them inherited the neutral floor's slate-blue accent.
        assert_ne!(
            light.primary_fixed,
            ColorScheme::neutral_light().primary_fixed
        );

        assert_eq!(light.inverse_primary, ShadcnBase::Neutral.dark().primary);
        assert_eq!(dark.inverse_primary, ShadcnBase::Neutral.light().primary);
    }

    #[test]
    fn error_container_is_the_destructive_wash_and_scrim_is_black_50() {
        let s = color_scheme_light();
        let destructive = ShadcnBase::Neutral.light().destructive;
        let wash = s.error_container.components;
        assert_eq!(
            [wash[0], wash[1], wash[2]],
            [
                destructive.components[0],
                destructive.components[1],
                destructive.components[2]
            ]
        );
        assert!((wash[3] - DESTRUCTIVE_WASH_ALPHA).abs() < 1e-6);
        assert_eq!(s.on_error_container, destructive);

        let scrim = s.scrim.components;
        assert_eq!([scrim[0], scrim[1], scrim[2]], [0.0, 0.0, 0.0]);
        assert!((scrim[3] - SCRIM_ALPHA).abs() < 1e-6);
        assert_eq!(s.shadow, Color::BLACK);
    }

    #[test]
    fn shape_scale_carries_the_shadcn_radius_ladder() {
        let s = shape_scale();
        let r = ShadcnRadius::shadcn();
        assert_eq!(s.none, 0.0);
        assert_eq!(s.extra_small, r.sm);
        assert_eq!(s.small, r.md);
        assert_eq!(s.medium, r.lg);
        assert_eq!(s.medium, RADIUS_BASE);
        assert_eq!(s.large, r.xl);
        assert_eq!(s.large_increased, r.xl2);
        assert_eq!(s.extra_large, r.xl3);
        assert_eq!(s.extra_large_increased, r.xl4);
        assert_eq!(s.extra_extra_large, r.xl4, "the top slot clamps to 4xl");
        assert!(s.full.is_infinite(), "`full` stays the pill sentinel");
        // The extension's copy of the ladder and the ShapeScale agree — the one
        // deliberate duplication stays pinned.
        assert_eq!(theme().shape.medium, ShadcnTokens::shadcn().radius.lg);
    }

    #[test]
    fn type_scale_names_the_bundled_sans_family_in_every_slot() {
        let scale = type_scale(&TextStyle::default());
        let expected = sans_family();
        for family in [
            &scale.display_large.family,
            &scale.headline_medium.family,
            &scale.title_large.family,
            &scale.body_medium.family,
            &scale.label_small.family,
            &scale.body_large_emphasized.family,
        ] {
            assert_eq!(family, &expected);
        }
        // Named face first, generic fallback last — so text still renders if a
        // host never drained the font registry.
        match sans_family() {
            FontFamily::NamedWithGeneric(parts) => {
                assert_eq!(parts.first(), Some(&FamilyName::Named(INTER_FAMILY.into())));
                assert_eq!(
                    parts.last(),
                    Some(&FamilyName::Generic(GenericSlot::SansSerif))
                );
            }
            other => panic!("expected a named+generic stack, got {other:?}"),
        }
        match mono_family() {
            FontFamily::NamedWithGeneric(parts) => {
                assert_eq!(
                    parts.first(),
                    Some(&FamilyName::Named(JETBRAINS_MONO_FAMILY.into()))
                );
                assert_eq!(
                    parts.last(),
                    Some(&FamilyName::Generic(GenericSlot::Monospace))
                );
            }
            other => panic!("expected a named+generic stack, got {other:?}"),
        }
        // Numbers stay the neutral scale's (family-only swap).
        let neutral = TypeScale::neutral(&TextStyle::default());
        assert_eq!(scale.body_medium.size, neutral.body_medium.size);
    }

    #[test]
    fn every_preset_constructor_builds_its_own_base() {
        let by_hand = [
            (theme_stone(), ShadcnBase::Stone),
            (theme_zinc(), ShadcnBase::Zinc),
            (theme_mauve(), ShadcnBase::Mauve),
            (theme_olive(), ShadcnBase::Olive),
            (theme_mist(), ShadcnBase::Mist),
            (theme_taupe(), ShadcnBase::Taupe),
        ];
        for (built, base) in by_hand {
            assert_eq!(built.light, theme_for(base).light, "{}", base.id());
            assert_eq!(built.light.on_surface, base.light().foreground);
            assert_eq!(
                built.extension::<ShadcnTokens>(),
                Some(&ShadcnTokens::for_base(base))
            );
            // Every preset shares the identity tag, shape scale and type scale —
            // only colors differ.
            assert_eq!(
                built.design_language,
                DesignLanguage::Custom(SHADCN_DESIGN_LANGUAGE)
            );
            assert_eq!(built.shape, shape_scale());
            assert_ne!(
                built.light,
                theme().light,
                "{} differs from neutral",
                base.id()
            );
        }
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn native_typefaces_bind_the_bundled_inter_bytes_by_identity() {
        // The native publish guard de-duplicates by pointer/length, so the
        // attached faces must BE `font_data()`'s entry, not a copy of it.
        let faces = theme()
            .extension::<NativeTypefaces>()
            .copied()
            .expect("the shadcn theme attaches NativeTypefaces");
        let bundled = fonts::font_data();
        let button = faces.button.expect("button slot is bound");
        let body = faces.body.expect("body slot is bound");
        assert_eq!(button.family, INTER_FAMILY);
        assert_eq!(body.family, INTER_FAMILY);
        assert!(std::ptr::eq(button.bytes, bundled[INTER_VARIABLE_INDEX]));
        assert!(std::ptr::eq(body.bytes, bundled[INTER_VARIABLE_INDEX]));
        // The mono face is registered but not bound to a native slot: no shadcn
        // native control asks for monospace.
        assert!(!std::ptr::eq(
            button.bytes,
            bundled[JETBRAINS_MONO_VARIABLE_INDEX]
        ));
    }
}
