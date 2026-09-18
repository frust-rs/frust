//! The Cupertino baseline design tokens this crate installs: a [`Theme`]
//! aggregate plus its constituent light/dark [`ColorScheme`]s, the Cupertino
//! type scale/shape scale/elevation table/motion scheme, and the
//! success/warning/info [`StatusPalette`] extension.
//!
//! These tables were lifted out of `frust-theme` when the Cupertino catalog
//! moved out of tree; nothing in the framework constructs them any more, so
//! this crate owns them outright. The Liquid Glass recipe lives in
//! [`crate::glass`] instead (a separate module — see that module's own docs
//! for why).

use frust::authoring::text::{FontFamily, FontWeight, GenericSlot, LineHeight, TextStyle};
use frust::{
    Brightness, ColorScheme, CosmeticLoopRate, Curve, DesignLanguage, EasingSet, Elevation,
    ElevationLevel, MotionDurations, MotionScheme, MotionSpring, ShadowSpec, ShapeScale,
    StatusColors, StatusPalette, SurfaceRole, Theme, ThemeExtensions, TypeScale,
};
use peniko::Color;

/// The Cupertino baseline theme: baseline light/dark Cupertino color
/// schemes, the Cupertino type scale (built from `TextStyle::default()`),
/// the Cupertino shape scale, the Cupertino elevation table, the Cupertino
/// motion scheme, and the [`crate::glass::ios27`] Liquid Glass scale. Starts
/// in [`Brightness::Light`]. Attaches [`status_palette`] as a pre-populated
/// extension (Cupertino has no published success/warning/info equivalent,
/// so it shares the Material 3 default rather than going unset — see
/// `Theme::extension`).
pub fn baseline() -> Theme {
    let mut extensions = ThemeExtensions::new();
    extensions.insert(status_palette());
    Theme {
        light: color_scheme_light(),
        dark: color_scheme_dark(),
        type_scale: type_scale(&TextStyle::default()),
        shape: shape_scale(),
        elevation: elevation(),
        motion: motion_scheme(),
        glass: crate::glass::ios27(),
        brightness: Brightness::Light,
        design_language: DesignLanguage::Cupertino,
        extensions,
    }
}

/// The Cupertino (iOS) light [`ColorScheme`] — semantic roles from Apple's
/// iOS semantic color palette, resolved against the iOS 27 UI Kit
/// (2026-07-18) where a real, citable kit record exists; a Frust-authored
/// fill-in otherwise (M3 concepts with no iOS equivalent — "container"
/// roles, the 5-step surface-container ladder).
pub fn color_scheme_light() -> ColorScheme {
    // systemBlue — community-measured (Apple doesn't publish exact
    // hex); iOS 27 UI Kit `System Colors/Light/8 Blue`, 2026-07-18.
    const SYSTEM_BLUE_LIGHT: Color = Color::from_rgb8(0x00, 0x87, 0xFF);
    const SYSTEM_BLUE_DARK: Color = Color::from_rgb8(0x00, 0x90, 0xFF);
    // systemPurple — community-measured, used as the "tertiary" accent;
    // iOS 27 UI Kit `System Colors/Light/10 Purple`, 2026-07-18.
    const SYSTEM_PURPLE_LIGHT: Color = Color::from_rgb8(0xCB, 0x2F, 0xE0);
    // systemGray — Apple documents the same base hex for light/dark;
    // iOS 27 UI Kit `Grays/Light|Dark/Gray`, 2026-07-18.
    const SYSTEM_GRAY: Color = Color::from_rgb8(0x8D, 0x8D, 0x92);
    // systemRed — community-measured; iOS 27 UI Kit
    // `System Colors/Light/1 Red`, 2026-07-18.
    const SYSTEM_RED_LIGHT: Color = Color::from_rgb8(0xFF, 0x38, 0x3C);
    const LABEL_LIGHT: Color = Color::from_rgb8(0x00, 0x00, 0x00);
    // opaqueSeparator (Apple-documented flattened hairline); iOS 27 UI
    // Kit `Separators/Light/Opaque`, 2026-07-18.
    const OPAQUE_SEPARATOR_LIGHT: Color = Color::from_rgb8(0xC5, 0xC5, 0xC7);

    ColorScheme {
        primary: SYSTEM_BLUE_LIGHT,
        on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        // Frust-derived tinted "container" (iOS has no container
        // role) — a pale tint of systemBlue over systemBackground.
        primary_container: Color::from_rgb8(0xD6, 0xE9, 0xFF),
        on_primary_container: SYSTEM_BLUE_LIGHT,
        // "Fixed" roles are brightness-invariant by M3 definition — the
        // light-mode tone reused verbatim in both color_scheme_light() and
        // color_scheme_dark().
        primary_fixed: SYSTEM_BLUE_LIGHT,
        primary_fixed_dim: SYSTEM_BLUE_DARK,
        on_primary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_primary_fixed_variant: Color::from_rgb8(0x00, 0x4C, 0x99),

        secondary: SYSTEM_GRAY,
        on_secondary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        secondary_container: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        on_secondary_container: LABEL_LIGHT,
        secondary_fixed: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        secondary_fixed_dim: Color::from_rgb8(0x2C, 0x2C, 0x2E),
        on_secondary_fixed: LABEL_LIGHT,
        on_secondary_fixed_variant: SYSTEM_GRAY,

        tertiary: SYSTEM_PURPLE_LIGHT,
        on_tertiary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        tertiary_container: Color::from_rgb8(0xF3, 0xE1, 0xFB),
        on_tertiary_container: SYSTEM_PURPLE_LIGHT,
        tertiary_fixed: SYSTEM_PURPLE_LIGHT,
        // iOS 27 UI Kit `System Colors/Dark/10 Purple`, 2026-07-18.
        tertiary_fixed_dim: Color::from_rgb8(0xDB, 0x34, 0xF1),
        on_tertiary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_tertiary_fixed_variant: Color::from_rgb8(0x6B, 0x2E, 0x86),

        error: SYSTEM_RED_LIGHT,
        on_error: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        error_container: Color::from_rgb8(0xFF, 0xD9, 0xD6),
        on_error_container: SYSTEM_RED_LIGHT,

        // systemBackground — iOS 27 UI Kit
        // `Backgrounds/Light - Base/Primary`, 2026-07-18 (confirmed
        // unchanged, opaque white).
        surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_surface: LABEL_LIGHT,
        // secondaryLabel — translucent per its real iOS token,
        // confirmed unchanged by `Labels/Light/2 Secondary`, 2026-07-18.
        on_surface_variant: Color::from_rgba8(0x3C, 0x3C, 0x43, 153),
        // secondarySystemBackground — iOS 27 UI Kit
        // `Backgrounds/Light - Base/Secondary`, 2026-07-18.
        surface_dim: Color::from_rgb8(0xF1, 0xF1, 0xF6),
        // tertiarySystemBackground — iOS 27 UI Kit
        // `Backgrounds/Light - Base/Tertiary`, 2026-07-18 (confirmed
        // unchanged, opaque white).
        surface_bright: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        // Frust-authored 5-step elevation ladder built from the iOS
        // background/gray tones — iOS has no native 5-step ladder concept.
        surface_container_lowest: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        surface_container_low: Color::from_rgb8(0xF7, 0xF7, 0xFA),
        surface_container: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        surface_container_high: Color::from_rgb8(0xE5, 0xE5, 0xEA),
        surface_container_highest: Color::from_rgb8(0xD1, 0xD1, 0xD6),

        outline: OPAQUE_SEPARATOR_LIGHT,
        // separator — translucent per its real iOS token; iOS 27 UI Kit
        // `Separators/Light/Non-Opaque`, 2026-07-18.
        outline_variant: Color::from_rgba8(0x00, 0x00, 0x00, 31),
        shadow: Color::from_rgb8(0x00, 0x00, 0x00),
        scrim: Color::from_rgb8(0x00, 0x00, 0x00),
        // "Inverse" roles reuse the opposite brightness's base tones.
        inverse_surface: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_on_surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        inverse_primary: SYSTEM_BLUE_DARK,
        // Mirrors M3's own convention: surface_tint == primary.
        surface_tint: SYSTEM_BLUE_LIGHT,
    }
}

/// The Cupertino (iOS) dark [`ColorScheme`] — the dark-mode mirror of
/// [`color_scheme_light`]; see that constructor's doc comment for sources.
pub fn color_scheme_dark() -> ColorScheme {
    // iOS 27 UI Kit `System Colors/Light|Dark/8 Blue`, 2026-07-18.
    const SYSTEM_BLUE_LIGHT: Color = Color::from_rgb8(0x00, 0x87, 0xFF);
    const SYSTEM_BLUE_DARK: Color = Color::from_rgb8(0x00, 0x90, 0xFF);
    // iOS 27 UI Kit `System Colors/Dark/10 Purple`, 2026-07-18.
    const SYSTEM_PURPLE_DARK: Color = Color::from_rgb8(0xDB, 0x34, 0xF1);
    // iOS 27 UI Kit `Grays/Light|Dark/Gray`, 2026-07-18.
    const SYSTEM_GRAY: Color = Color::from_rgb8(0x8D, 0x8D, 0x92);
    // iOS 27 UI Kit `System Colors/Dark/1 Red`, 2026-07-18.
    const SYSTEM_RED_DARK: Color = Color::from_rgb8(0xFF, 0x42, 0x45);
    const LABEL_DARK: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
    // opaqueSeparator (Apple-documented flattened hairline), dark; iOS
    // 27 UI Kit `Separators/Dark/Opaque`, 2026-07-18.
    const OPAQUE_SEPARATOR_DARK: Color = Color::from_rgb8(0x38, 0x38, 0x3A);

    ColorScheme {
        primary: SYSTEM_BLUE_DARK,
        on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        primary_container: Color::from_rgb8(0x16, 0x3A, 0x5C),
        on_primary_container: SYSTEM_BLUE_DARK,
        primary_fixed: SYSTEM_BLUE_LIGHT,
        primary_fixed_dim: SYSTEM_BLUE_DARK,
        on_primary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_primary_fixed_variant: Color::from_rgb8(0x00, 0x4C, 0x99),

        secondary: SYSTEM_GRAY,
        on_secondary: Color::from_rgb8(0x00, 0x00, 0x00),
        secondary_container: Color::from_rgb8(0x1C, 0x1C, 0x1E),
        on_secondary_container: LABEL_DARK,
        secondary_fixed: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        secondary_fixed_dim: Color::from_rgb8(0x2C, 0x2C, 0x2E),
        on_secondary_fixed: Color::from_rgb8(0x00, 0x00, 0x00),
        on_secondary_fixed_variant: SYSTEM_GRAY,

        tertiary: SYSTEM_PURPLE_DARK,
        on_tertiary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        tertiary_container: Color::from_rgb8(0x3B, 0x1F, 0x49),
        on_tertiary_container: SYSTEM_PURPLE_DARK,
        // iOS 27 UI Kit `System Colors/Light/10 Purple`, 2026-07-18.
        tertiary_fixed: Color::from_rgb8(0xCB, 0x2F, 0xE0),
        tertiary_fixed_dim: SYSTEM_PURPLE_DARK,
        on_tertiary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_tertiary_fixed_variant: Color::from_rgb8(0x6B, 0x2E, 0x86),

        error: SYSTEM_RED_DARK,
        on_error: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        error_container: Color::from_rgb8(0x4C, 0x16, 0x13),
        on_error_container: SYSTEM_RED_DARK,

        // systemBackground, dark — iOS 27 UI Kit
        // `Backgrounds/Dark - Base/Primary`, 2026-07-18 (confirmed
        // unchanged, opaque black).
        surface: Color::from_rgb8(0x00, 0x00, 0x00),
        on_surface: LABEL_DARK,
        // secondaryLabel, dark — translucent per its real iOS token;
        // iOS 27 UI Kit `Labels/Dark/2 Secondary`, 2026-07-18.
        on_surface_variant: Color::from_rgba8(0xEA, 0xEA, 0xF4, 153),
        // secondarySystemBackground, dark — iOS 27 UI Kit
        // `Backgrounds/Dark - Base/Secondary`, 2026-07-18.
        surface_dim: Color::from_rgb8(0x1B, 0x1B, 0x1D),
        // tertiarySystemBackground, dark — the "brightest" dark surface;
        // iOS 27 UI Kit `Backgrounds/Dark - Base/Tertiary`, 2026-07-18.
        surface_bright: Color::from_rgb8(0x2B, 0x2B, 0x2D),
        surface_container_lowest: Color::from_rgb8(0x00, 0x00, 0x00),
        surface_container_low: Color::from_rgb8(0x1C, 0x1C, 0x1E),
        surface_container: Color::from_rgb8(0x2C, 0x2C, 0x2E),
        surface_container_high: Color::from_rgb8(0x3A, 0x3A, 0x3C),
        surface_container_highest: Color::from_rgb8(0x48, 0x48, 0x4A),

        outline: OPAQUE_SEPARATOR_DARK,
        // separator, dark — translucent per its real iOS token; iOS 27
        // UI Kit `Separators/Dark/Non-Opaque`, 2026-07-18.
        outline_variant: Color::from_rgba8(0xFF, 0xFF, 0xFF, 31),
        shadow: Color::from_rgb8(0x00, 0x00, 0x00),
        scrim: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        inverse_on_surface: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_primary: SYSTEM_BLUE_LIGHT,
        surface_tint: SYSTEM_BLUE_DARK,
    }
}

/// The Material 3 baseline [`StatusPalette`] default, shared by the
/// Cupertino baseline (see [`baseline`]'s doc comment for why): Cupertino
/// has no published success/warning/info equivalent either.
///
/// **Community-approximate**: Material 3 has no single official token source
/// for a `success`/`warning`/`info` role table — the values below apply the
/// same tone-relationship `color_scheme_light`/`color_scheme_dark`'s `error`
/// roles use (light: base/on/container/on-container ≈ tone 40/100/90/10;
/// dark: ≈ tone 80/20/30/90) to green (success), amber (warning), and blue
/// (info) seed hues, chosen for conventional semantic association and AA
/// contrast against `surface`/`on_surface`.
pub fn status_palette() -> StatusPalette {
    StatusPalette {
        light: StatusColors {
            // Green seed, ~tone 40/100/90/10 (mirrors `error`'s light
            // tone relationship: base/on/container/on-container).
            success: Color::from_rgb8(0x2E, 0x7D, 0x32),
            on_success: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            success_container: Color::from_rgb8(0xC8, 0xE6, 0xC9),
            on_success_container: Color::from_rgb8(0x1B, 0x5E, 0x20),

            // Amber seed, tuned dark enough for AA-on-white at the base
            // tone (a literal amber-400 like `#FFC107` fails AA on white).
            warning: Color::from_rgb8(0x8A, 0x53, 0x00),
            on_warning: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            warning_container: Color::from_rgb8(0xFF, 0xDD, 0xB0),
            on_warning_container: Color::from_rgb8(0x2B, 0x17, 0x00),

            // Blue seed, echoing M3's own `primary`-adjacent "info" blue
            // used elsewhere in Google's Material guidance.
            info: Color::from_rgb8(0x00, 0x61, 0xA4),
            on_info: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            info_container: Color::from_rgb8(0xD1, 0xE4, 0xFF),
            on_info_container: Color::from_rgb8(0x00, 0x1D, 0x36),
        },
        dark: StatusColors {
            // Dark tone relationship (~80/20/30/90), mirroring `error`'s
            // dark tones (`F2B8B5`/`601410`/`8C1D18`/`F9DEDC`).
            success: Color::from_rgb8(0xA6, 0xF1, 0xA1),
            on_success: Color::from_rgb8(0x00, 0x39, 0x0F),
            success_container: Color::from_rgb8(0x20, 0x57, 0x23),
            on_success_container: Color::from_rgb8(0xC8, 0xE6, 0xC9),

            warning: Color::from_rgb8(0xFF, 0xC4, 0x6B),
            on_warning: Color::from_rgb8(0x45, 0x2B, 0x00),
            warning_container: Color::from_rgb8(0x6F, 0x49, 0x00),
            on_warning_container: Color::from_rgb8(0xFF, 0xDD, 0xB0),

            info: Color::from_rgb8(0x9F, 0xCA, 0xFF),
            on_info: Color::from_rgb8(0x00, 0x32, 0x50),
            info_container: Color::from_rgb8(0x00, 0x4A, 0x76),
            on_info_container: Color::from_rgb8(0xD1, 0xE4, 0xFF),
        },
    }
}

// ---- Type scale -------------------------------------------------------

/// [`type_scale`] maps Apple's 11 SF Pro semantic text styles onto the same
/// 15 M3-named [`TypeScale`] slots (retrieved 2026-07-17):
///
/// - **Sizes** (pt, treated 1:1 as logical px like the M3 scale's sp): Large
///   Title 34, Title 1 28, Title 2 22, Title 3 20, Headline 17, Body 17,
///   Callout 16, Subheadline 15, Footnote 13, Caption 1 12, Caption 2 11.
/// - **Weights (correction):** Apple's default text-style weight is
///   Regular for every style *except* Headline, which is Semibold — Large
///   Title/Title 1/Title 2/Title 3 are Regular by default, not Bold/Semibold
///   as an earlier community table claimed; Bold/Semibold variants of those
///   styles exist only as an opt-in "emphasized" style, not the default.
/// - **Family (correction):** SF Pro switches between the "Text" optical
///   size (≤19pt, more open spacing) and "Display" optical size (≥20pt,
///   tighter spacing) purely by point size, with **no dpi/density
///   component** (an earlier community "144dpi" cutoff claim is unsupported
///   by Apple documentation). Each slot names its SF Pro style first, then
///   falls back to the platform's system UI font — SF Pro's license keeps
///   it iOS-only, so an unresolved name lands on the system UI face, never
///   on whatever font parley's own unmatched-name resolution would
///   otherwise pick (Helvetica, on macOS).
/// - **Letter spacing:** left at `0.0` for every Cupertino token — SF Pro's
///   per-size optical tracking is baked into the font's own metrics tables
///   (applied by CoreText/parley from the font itself), not an
///   app-superimposed additional value the way M3's type scale specifies
///   one.
/// - **Line height:** Apple's official HIG leading-per-style table is only
///   partly confirmed (some of the commonly-cited leading figures —
///   Callout/Footnote/Caption1/Caption2 — diverge from the archived official
///   table); the values below are the widely-cited **community-approximate**
///   absolute leading (pt) per style, not a source Apple itself currently
///   publishes verbatim.
/// - **Slot mapping:** 11 source styles must fill 15 M3 slots, so 4 slots
///   necessarily reuse an adjacent style's exact size/weight/leading — a
///   Frust editorial choice (documented here, not implying a real 1:1
///   Apple/M3 crosswalk exists) rather than an invented intermediate size:
///
///   | M3 slot          | SF Pro style   | size | weight   |
///   |------------------|----------------|------|----------|
///   | `display_large`  | Large Title    | 34   | Regular  |
///   | `display_medium` | Large Title    | 34   | Regular  |
///   | `display_small`  | Title 1        | 28   | Regular  |
///   | `headline_large` | Title 1        | 28   | Regular  |
///   | `headline_medium`| Title 2        | 22   | Regular  |
///   | `headline_small` | Title 3        | 20   | Regular  |
///   | `title_large`    | Headline       | 17   | Semibold |
///   | `title_medium`   | Headline       | 17   | Semibold |
///   | `title_small`    | Body           | 17   | Regular  |
///   | `body_large`     | Body           | 17   | Regular  |
///   | `body_medium`    | Callout        | 16   | Regular  |
///   | `body_small`     | Subheadline    | 15   | Regular  |
///   | `label_large`    | Footnote       | 13   | Regular  |
///   | `label_medium`   | Caption 1      | 12   | Regular  |
///   | `label_small`    | Caption 2      | 11   | Regular  |
///
///   The Text/Display family cutoff falls exactly at the `headline`/`title`
///   boundary above (20pt vs 17pt), so every `display_*`/`headline_*` slot
///   uses the Display stack and every `title_*`/`body_*`/`label_*` slot uses
///   the Text stack.
///
/// **Emphasized slots:** SF Pro weights mined from the Apple iOS UI Kit
/// (`text_styles` — only Regular and Semibold appear); [`type_scale`]'s
/// emphasized slots reuse the same size/line-height/family as their baseline
/// counterpart and force weight to Semibold — including `title_large`/
/// `title_medium` (mapped from SF Pro Headline, already Semibold by
/// default), whose emphasized variant is therefore numerically identical to
/// its own baseline; this is a faithful mapping outcome (SF Pro has no
/// weight above Semibold to step up to here), not an oversight.
/// SF Pro's Text/Display optical-size cutoff, in pt: sizes `< 20.0` use "SF
/// Pro Text", sizes `>= 20.0` use "SF Pro Display" — point-size driven only,
/// no dpi component (see doc comment above).
const TEXT_DISPLAY_CUTOFF_PT: f32 = 20.0;

/// One Cupertino type-scale token's numeric shape: `(size_pt,
/// line_height_pt, weight)`. Letter spacing is always `0.0` (see module
/// docs); family is derived from `size_pt` against
/// [`TEXT_DISPLAY_CUTOFF_PT`], not stored here.
type TypeToken = (f32, f32, FontWeight);

// SF Pro semantic text styles used above the M3 slots (see the doc comment
// above's mapping table): sizes/weights per Apple's official defaults (only
// Headline is Semibold); line heights are the widely-cited
// **community-approximate** absolute leading per style (some figures are
// unconfirmed against the archived official table — see doc comment above).
const DISPLAY_LARGE: TypeToken = (34.0, 41.0, FontWeight::REGULAR); // Large Title
const DISPLAY_MEDIUM: TypeToken = (34.0, 41.0, FontWeight::REGULAR); // Large Title
const DISPLAY_SMALL: TypeToken = (28.0, 34.0, FontWeight::REGULAR); // Title 1
const HEADLINE_LARGE: TypeToken = (28.0, 34.0, FontWeight::REGULAR); // Title 1
const HEADLINE_MEDIUM: TypeToken = (22.0, 28.0, FontWeight::REGULAR); // Title 2
const HEADLINE_SMALL: TypeToken = (20.0, 25.0, FontWeight::REGULAR); // Title 3
const TITLE_LARGE: TypeToken = (17.0, 22.0, FontWeight::SEMI_BOLD); // Headline
const TITLE_MEDIUM: TypeToken = (17.0, 22.0, FontWeight::SEMI_BOLD); // Headline
const TITLE_SMALL: TypeToken = (17.0, 22.0, FontWeight::REGULAR); // Body
const BODY_LARGE: TypeToken = (17.0, 22.0, FontWeight::REGULAR); // Body
const BODY_MEDIUM: TypeToken = (16.0, 21.0, FontWeight::REGULAR); // Callout
const BODY_SMALL: TypeToken = (15.0, 20.0, FontWeight::REGULAR); // Subheadline
const LABEL_LARGE: TypeToken = (13.0, 18.0, FontWeight::REGULAR); // Footnote
const LABEL_MEDIUM: TypeToken = (12.0, 16.0, FontWeight::REGULAR); // Caption 1
const LABEL_SMALL: TypeToken = (11.0, 13.0, FontWeight::REGULAR); // Caption 2

/// The SF Pro family stack for `size_pt`, switching at
/// [`TEXT_DISPLAY_CUTOFF_PT`] (see the module doc's Family bullet). Ends in
/// [`GenericSlot::SystemUi`], so an unresolved SF Pro name falls back to
/// the platform's system UI font rather than parley's own arbitrary
/// unmatched-name resolution.
fn sf_family_for_size(size_pt: f32) -> FontFamily {
    if size_pt >= TEXT_DISPLAY_CUTOFF_PT {
        FontFamily::stack_with_generic(["SF Pro Display", "SF Pro"], GenericSlot::SystemUi)
    } else {
        FontFamily::stack_with_generic(["SF Pro Text", "SF Pro"], GenericSlot::SystemUi)
    }
}

fn apply_type(base: &TextStyle, token: TypeToken) -> TextStyle {
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

/// Builds an emphasized token from its baseline `token`'s size/line-height
/// (unchanged), forcing weight to Semibold — the only emphasized weight SF
/// Pro's mined text styles (Regular, Semibold) support (see the doc comment
/// above).
fn apply_type_emphasized(base: &TextStyle, token: TypeToken) -> TextStyle {
    let (size, line_height_pt, _weight) = token;
    apply_type(base, (size, line_height_pt, FontWeight::SEMI_BOLD))
}

/// Builds the Cupertino (iOS) type scale from `base` (its `style`/`color`
/// are preserved on every token; `size`/`weight`/`letter_spacing` (always
/// `0.0`)/`line_height`/`family` are all Cupertino-specified — `family` is
/// overridden per slot too, since the Text/Display optical-size switch (see
/// doc comment above) needs to be chosen per token, not inherited from
/// `base`).
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
        display_large_emphasized: apply_type_emphasized(base, DISPLAY_LARGE),
        display_medium_emphasized: apply_type_emphasized(base, DISPLAY_MEDIUM),
        display_small_emphasized: apply_type_emphasized(base, DISPLAY_SMALL),
        headline_large_emphasized: apply_type_emphasized(base, HEADLINE_LARGE),
        headline_medium_emphasized: apply_type_emphasized(base, HEADLINE_MEDIUM),
        headline_small_emphasized: apply_type_emphasized(base, HEADLINE_SMALL),
        title_large_emphasized: apply_type_emphasized(base, TITLE_LARGE),
        title_medium_emphasized: apply_type_emphasized(base, TITLE_MEDIUM),
        title_small_emphasized: apply_type_emphasized(base, TITLE_SMALL),
        body_large_emphasized: apply_type_emphasized(base, BODY_LARGE),
        body_medium_emphasized: apply_type_emphasized(base, BODY_MEDIUM),
        body_small_emphasized: apply_type_emphasized(base, BODY_SMALL),
        label_large_emphasized: apply_type_emphasized(base, LABEL_LARGE),
        label_medium_emphasized: apply_type_emphasized(base, LABEL_MEDIUM),
        label_small_emphasized: apply_type_emphasized(base, LABEL_SMALL),
    }
}

// ---- Shape scale --------------------------------------------------------

/// The Cupertino (iOS) shape scale: 10 corner-radius tokens. Apple does not
/// publish a numeric corner-radius scale the way M3 does — every value below
/// is a **community-approximate** convention (retrieved 2026-07-17): `small`
/// (8pt) is the widely-cited standard `UIButton`/control corner radius;
/// `medium` (10pt) approximates a text field/small card;
/// `large`/`large_increased` (13pt/14pt) bracket the community-cited
/// alert/action-sheet radius range (Apple's own HIG text does not publish
/// this figure); `extra_large`/`extra_large_increased` (20pt/24pt)
/// approximate a sheet or large modal card; `extra_extra_large` (36pt) has no
/// iOS precedent at all and is a Frust linear extrapolation to fill the
/// scale's largest slot. **iOS corners are visually "continuous"
/// (superellipse/"squircle") curves, not circular arcs** — a numeric radius
/// here approximates the visual size of that curve, not an exact geometric
/// equivalent; Frust's own corner painting (a circular-arc rounded rect)
/// does not reproduce the continuous curve shape, only its rough footprint.
pub const fn shape_scale() -> ShapeScale {
    ShapeScale {
        none: 0.0,
        extra_small: 4.0,
        small: 8.0,
        medium: 10.0,
        large: 13.0,
        large_increased: 14.0,
        extra_large: 20.0,
        extra_large_increased: 24.0,
        extra_extra_large: 36.0,
        full: f64::INFINITY,
    }
}

// ---- Elevation ----------------------------------------------------------

/// The Cupertino v1 shadow mapping: `y_offset = dp / 4.0`, `blur_std_dev =
/// dp * 0.6`, `color_alpha = 0.12` — iOS shadows are typically much
/// softer/lower-contrast than Android's Material shadows (community
/// convention, not an Apple-published spec — TUNABLE, not load-bearing,
/// Frust-specific policy, same caveat the M3 mapping this crate's sibling
/// carries).
const fn elevation_level(dp: f64, surface_role: SurfaceRole) -> ElevationLevel {
    let shadow = ShadowSpec {
        y_offset: dp / 4.0,
        blur_std_dev: dp * 0.6,
        color_alpha: 0.12,
    };
    ElevationLevel {
        dp,
        shadow_light: shadow,
        shadow_dark: shadow,
        surface_role,
    }
}

/// The Cupertino (iOS) elevation table — the same 6-level/dp ladder and
/// `SurfaceRole` assignment as Material 3's own elevation table (iOS has no
/// published elevation-level system of its own to source a different ladder
/// from either), with the subtler v1 shadow math in [`elevation_level`]'s
/// doc comment. dp source: <https://m3.material.io/styles/elevation>
/// (verified 2026-07-17): L0 0dp, L1 1dp, L2 3dp, L3 6dp, L4 8dp, L5 12dp.
///
/// [`ElevationLevel`] carries **separate** light/dark shadow specs; this
/// mapping doesn't branch by brightness, so both slots hold the same value —
/// behavior-preserving, byte-identical rendered output on either brightness.
pub const fn elevation() -> Elevation {
    Elevation {
        level0: elevation_level(0.0, SurfaceRole::Surface),
        level1: elevation_level(1.0, SurfaceRole::SurfaceContainerLow),
        level2: elevation_level(3.0, SurfaceRole::SurfaceContainer),
        level3: elevation_level(6.0, SurfaceRole::SurfaceContainerHigh),
        level4: elevation_level(8.0, SurfaceRole::SurfaceContainerHigh),
        level5: elevation_level(12.0, SurfaceRole::SurfaceContainerHighest),
    }
}

// ---- Motion ---------------------------------------------------------------

/// The Cupertino (iOS) motion scheme — the single community-documented iOS
/// spring baseline (retrieved 2026-07-17): mass `1.0`, stiffness `170.0`,
/// damping (coefficient, not ratio) `15.0` — the "start with a damping of 15
/// and a stiffness of 170" convention cited across multiple SwiftUI
/// community sources, applied uniformly to all six spring slots.
/// [`MotionSpring`] stores a damping *ratio* ζ (mass implicitly `1.0`), so
/// this converts the community coefficient via the standard relation `ζ = c
/// / (2 * sqrt(k * m))`: `15.0 / (2 * sqrt(170.0)) ≈ 0.5753` — an
/// under-damped spring with a gentle overshoot, consistent with iOS's
/// typically "springy" motion feel.
///
/// Unlike M3, **iOS has no published/community-documented distinction
/// between "fast"/"default"/"slow" speed tiers or "spatial"/"effects" motion
/// categories** — the single baseline above is the only community-converged
/// iOS spring value found. Rather than inventing an unsourced scaling factor
/// to differentiate the six slots, this applies the single documented
/// baseline **uniformly** to all six — a documented simplification (flagged
/// here, not a hidden gap), not a fabricated iOS-specific tuning
/// distinction.
///
/// **Community-approximate** duration/easing tokens: unlike M3, iOS
/// publishes no named multi-tier duration scale or curve-category
/// vocabulary — UIKit/SwiftUI animations are conventionally spring-authored
/// (the six presets above), not bezier-timed. The one citable data point
/// (retrieved 2026-07-17, citing Apple's WWDC23 "Animate with springs"
/// talk): SwiftUI's `.bouncy` spring preset defaults to a 0.5s duration —
/// anchoring `slow` at 500ms here. The remaining four slots
/// (`instant`/`fast`/`base`/`deliberate`) are a community-approximate scale
/// around that anchor, loosely following commonly-cited iOS interaction
/// timings (a ~0.2s "quick" feel, a ~0.35s modal-presentation-adjacent
/// "base" feel); replace them if a citable per-tier iOS source turns up. The
/// easing vocabulary has the same gap as the springs: no published
/// spatial/effects/exit distinction, so [`Curve::EaseInOut`] (UIKit's
/// default `UIView.AnimationOptions` curve) is applied uniformly to all
/// three easing slots, mirroring the springs' uniform-baseline treatment
/// above.
///
/// `cosmetic_loop_rate` is this design system's own authored value — 30Hz,
/// the same cap every built-in baseline declares.
pub const fn motion_scheme() -> MotionScheme {
    // ζ = 15.0 / (2 * sqrt(170.0)) ≈ 0.5753 (see doc comment above; sqrt
    // isn't const-evaluable on stable Rust, so the ratio is precomputed
    // here).
    const DAMPING_RATIO: f64 = 0.5753;
    const STIFFNESS: f64 = 170.0;
    let spring = MotionSpring {
        damping_ratio: DAMPING_RATIO,
        stiffness: STIFFNESS,
    };
    MotionScheme {
        fast_spatial: spring,
        fast_effects: spring,
        default_spatial: spring,
        default_effects: spring,
        slow_spatial: spring,
        slow_effects: spring,
        durations: MotionDurations {
            instant: 100.0,
            fast: 200.0,
            base: 350.0,
            slow: 500.0,
            deliberate: 700.0,
        },
        easing: EasingSet {
            spatial: Curve::EaseInOut,
            effects: Curve::EaseInOut,
            exit: Curve::EaseInOut,
        },
        reduce_motion: false,
        cosmetic_loop_rate: CosmeticLoopRate::new(30.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::FamilyName;

    #[test]
    fn type_scale_display_large_matches_large_title() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large.size, 34.0);
        assert_eq!(scale.display_large.line_height, LineHeight::Absolute(41.0));
        assert_eq!(scale.display_large.letter_spacing, 0.0);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn type_scale_only_title_slots_are_semibold() {
        // Only Headline defaults to Semibold; every other SF Pro style
        // (including Large Title/Title 1/2/3) is Regular.
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.title_large.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.title_medium.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
        assert_eq!(scale.headline_small.weight, FontWeight::REGULAR);
        assert_eq!(scale.title_small.weight, FontWeight::REGULAR);
        assert_eq!(scale.body_large.weight, FontWeight::REGULAR);
        assert_eq!(scale.label_small.weight, FontWeight::REGULAR);
    }

    #[test]
    fn type_scale_family_switches_at_the_text_display_cutoff() {
        // The Text/Display switch is size-driven — >= 20pt uses Display,
        // < 20pt uses Text — with no dpi component. Both stacks carry the
        // SystemUi generic tail (see `sf_family_for_size`'s doc comment).
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        let display_stack =
            FontFamily::stack_with_generic(["SF Pro Display", "SF Pro"], GenericSlot::SystemUi);
        let text_stack =
            FontFamily::stack_with_generic(["SF Pro Text", "SF Pro"], GenericSlot::SystemUi);

        assert_eq!(scale.display_large.family, display_stack); // 34pt
        assert_eq!(scale.headline_small.family, display_stack); // 20pt: boundary, Display
        assert_eq!(scale.title_large.family, text_stack); // 17pt
        assert_eq!(scale.label_small.family, text_stack); // 11pt
    }

    /// Guards every [`TypeScale`] role's family against losing its
    /// [`GenericSlot::SystemUi`] tail, the fallback that keeps an unresolved
    /// SF Pro name on the platform's system UI font.
    #[test]
    fn every_type_scale_slot_ends_in_the_system_ui_generic_tail() {
        fn ends_in_system_ui(family: &FontFamily) -> bool {
            matches!(
                family,
                FontFamily::NamedWithGeneric(names)
                    if matches!(names.last(), Some(FamilyName::Generic(GenericSlot::SystemUi)))
            )
        }

        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        let slots: [(&str, &TextStyle); 30] = [
            ("display_large", &scale.display_large),
            ("display_medium", &scale.display_medium),
            ("display_small", &scale.display_small),
            ("headline_large", &scale.headline_large),
            ("headline_medium", &scale.headline_medium),
            ("headline_small", &scale.headline_small),
            ("title_large", &scale.title_large),
            ("title_medium", &scale.title_medium),
            ("title_small", &scale.title_small),
            ("body_large", &scale.body_large),
            ("body_medium", &scale.body_medium),
            ("body_small", &scale.body_small),
            ("label_large", &scale.label_large),
            ("label_medium", &scale.label_medium),
            ("label_small", &scale.label_small),
            ("display_large_emphasized", &scale.display_large_emphasized),
            (
                "display_medium_emphasized",
                &scale.display_medium_emphasized,
            ),
            ("display_small_emphasized", &scale.display_small_emphasized),
            (
                "headline_large_emphasized",
                &scale.headline_large_emphasized,
            ),
            (
                "headline_medium_emphasized",
                &scale.headline_medium_emphasized,
            ),
            (
                "headline_small_emphasized",
                &scale.headline_small_emphasized,
            ),
            ("title_large_emphasized", &scale.title_large_emphasized),
            ("title_medium_emphasized", &scale.title_medium_emphasized),
            ("title_small_emphasized", &scale.title_small_emphasized),
            ("body_large_emphasized", &scale.body_large_emphasized),
            ("body_medium_emphasized", &scale.body_medium_emphasized),
            ("body_small_emphasized", &scale.body_small_emphasized),
            ("label_large_emphasized", &scale.label_large_emphasized),
            ("label_medium_emphasized", &scale.label_medium_emphasized),
            ("label_small_emphasized", &scale.label_small_emphasized),
        ];

        for (name, style) in slots {
            assert!(
                ends_in_system_ui(&style.family),
                "type_scale.{name}.family does not end in GenericSlot::SystemUi: {:?} — \
                 an unresolved SF Pro name would fall back to parley's own arbitrary \
                 resolution instead of the platform system UI font",
                style.family
            );
        }
    }

    #[test]
    fn type_scale_preserves_base_style_and_color_but_overrides_family() {
        let base = TextStyle {
            family: FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = type_scale(&base);
        assert_ne!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.color, base.color);
        assert_eq!(scale.body_large.style, base.style);
    }

    #[test]
    fn type_scale_emphasized_slots_are_all_semibold() {
        // Emphasized forces Semibold across every slot — SF Pro's mined text
        // styles have no weight above Semibold to step up to.
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large_emphasized.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.body_large_emphasized.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.label_small_emphasized.weight, FontWeight::SEMI_BOLD);
        // title_large/title_medium map from SF Pro Headline, already
        // Semibold by default — their emphasized sibling is therefore
        // numerically identical to base (a faithful mapping outcome, not a
        // bug).
        assert_eq!(scale.title_large.weight, FontWeight::SEMI_BOLD);
        assert_eq!(scale.title_large_emphasized.weight, FontWeight::SEMI_BOLD);
    }

    #[test]
    fn type_scale_emphasized_keeps_base_size_and_family() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.body_large.size, scale.body_large_emphasized.size);
        assert_eq!(scale.body_large.family, scale.body_large_emphasized.family);
        assert_eq!(
            scale.body_large.line_height,
            scale.body_large_emphasized.line_height
        );
    }

    #[test]
    fn shape_scale_matches_table() {
        let s = shape_scale();
        assert_eq!(s.none, 0.0);
        assert_eq!(s.small, 8.0);
        assert_eq!(s.medium, 10.0);
        assert_eq!(s.large, 13.0);
        assert_eq!(s.large_increased, 14.0);
        assert_eq!(s.extra_large, 20.0);
        assert_eq!(s.extra_large_increased, 24.0);
        assert_eq!(s.extra_extra_large, 36.0);
        assert!(s.full.is_infinite());
    }

    #[test]
    fn elevation_dp_ladder_matches_m3() {
        // Same dp ladder as Material 3's own table — only the shadow math
        // and (not tested here, unchanged) surface-role assignment differ.
        let e = elevation();
        assert_eq!(e.level0.dp, 0.0);
        assert_eq!(e.level3.dp, 6.0);
        assert_eq!(e.level5.dp, 12.0);
    }

    #[test]
    fn elevation_shadow_is_subtler_than_m3() {
        let e = elevation();
        assert_eq!(e.level3.shadow_light.y_offset, 1.5);
        assert!((e.level3.shadow_light.blur_std_dev - 3.6).abs() < 1e-9);
        assert_eq!(e.level3.shadow_light.color_alpha, 0.12);
    }

    #[test]
    fn elevation_shadow_is_identical_on_both_brightnesses() {
        let e = elevation();
        assert_eq!(e.level3.shadow_light, e.level3.shadow_dark);
    }

    #[test]
    fn motion_scheme_uses_the_single_documented_baseline_uniformly() {
        let m = motion_scheme();
        let expected = MotionSpring {
            damping_ratio: 0.5753,
            stiffness: 170.0,
        };
        assert_eq!(m.fast_spatial, expected);
        assert_eq!(m.fast_effects, expected);
        assert_eq!(m.default_spatial, expected);
        assert_eq!(m.default_effects, expected);
        assert_eq!(m.slow_spatial, expected);
        assert_eq!(m.slow_effects, expected);
    }

    #[test]
    fn motion_scheme_damping_ratio_is_under_damped() {
        // ζ ≈ 0.5753 < 1.0 — a gentle overshoot, matching iOS's springy feel.
        let m = motion_scheme();
        assert!(m.default_spatial.damping_ratio < 1.0);
        assert!(m.default_spatial.damping_ratio > 0.0);
    }

    #[test]
    fn motion_scheme_cosmetic_loop_rate_is_30hz() {
        assert_eq!(motion_scheme().cosmetic_loop_rate.hz(), 30.0);
    }

    #[test]
    fn baseline_composes_the_cupertino_scales() {
        let theme = baseline();
        assert_eq!(theme.shape, shape_scale());
        assert_eq!(theme.elevation, elevation());
        assert_eq!(theme.motion, motion_scheme());
    }
}

/// Render-time typeface probe shared by every Cupertino text site's
/// render-time font-bytes tests (`action_sheet`, `alert_dialog`, `button`,
/// `navbar`, `tabbar`): paints a view through a real
/// [`frust_core::RenderRoot`] under a given [`Theme`] and reports every
/// painted glyph run's exact font bytes, so a site test can prove its
/// opted-in [`frust::authoring::ThemeTextType`] role — not
/// `TextStyle::default()`'s `SystemUi` — decided what shaped, the same way
/// `frust-material`'s own `typeface_probe`
/// (`plugins/material/src/appbar/top.rs`) does for its bundled Roboto Flex.
///
/// Cupertino's own scale resolves every role to a NAMED "SF Pro
/// Text"/"SF Pro Display" family stack ([`type_scale`]'s doc comment): that
/// name only actually resolves on iOS (SF Pro's license forbids
/// cross-platform bundling), so a render-time identity test here cannot
/// compare against real SF Pro bytes the way Material's bundled Roboto Flex
/// can. Instead a caller overrides the ONE role its site opts into
/// ([`baseline_with_family`]) to a [`FontFamily::named`] pointing at a
/// registered test face — [`TUFFY`]/[`TUFFY_AS_HELVETICA`], the same
/// public-domain cross-crate fixture `frust-widgets`' own themed-family
/// tests use (`crates/frust-text/tests/fonts/`, included the same
/// cross-crate way `crates/frust-widgets/src/text.rs`'s tests do) — leaving
/// every other Cupertino baseline token (color, shape, motion, glass)
/// untouched, and this probe proves every painted glyph run shaped against
/// it.
#[cfg(test)]
pub(crate) mod typeface_probe {
    use super::baseline;
    use frust::Theme;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::{FontFamily, TextContext};
    use frust::authoring::{PaintScene, View};
    use frust::text;
    use frust_core::{FrameTime, RenderRoot};
    use kurbo::{Point, Size};
    use peniko::Color;
    use std::any::Any;

    /// The public-domain subsetted test face `frust-text`'s own registration
    /// tests use, included cross-crate like `crates/frust-widgets/src/text.rs`'s
    /// identical fixture — registers as family name `"Tuffy"`.
    pub(crate) const TUFFY: &[u8] =
        include_bytes!("../../../crates/frust-text/tests/fonts/Tuffy-Subset.ttf");
    /// The same face with its internal name table renamed — distinct bytes
    /// from [`TUFFY`], registering as family name `"Helvetica"`, so a
    /// theme-swap test can tell which face a run shaped against.
    pub(crate) const TUFFY_AS_HELVETICA: &[u8] =
        include_bytes!("../../../crates/frust-text/tests/fonts/Tuffy-As-Helvetica.ttf");

    /// [`baseline`] with one [`frust::TypeScale`] slot's family swapped, via
    /// `set_family`, to a [`FontFamily::named`] pointing at `family_name`
    /// (usually [`TUFFY`]'s `"Tuffy"` or [`TUFFY_AS_HELVETICA`]'s
    /// `"Helvetica"` — register the matching bytes into the `TextContext` a
    /// caller lays out with). Every other Cupertino baseline token stays
    /// exactly [`baseline`]'s own.
    pub(crate) fn baseline_with_family(
        set_family: impl FnOnce(&mut frust::TypeScale, FontFamily),
        family_name: &str,
    ) -> Theme {
        let mut theme = baseline();
        set_family(&mut theme.type_scale, FontFamily::named(family_name));
        theme
    }

    /// Records each painted glyph run's exact font bytes, in paint order.
    #[derive(Default)]
    struct FaceRecorder {
        runs: Vec<Vec<u8>>,
    }

    impl PaintScene for FaceRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.runs.push(run.font.font().data.as_ref().to_vec());
        }
    }

    /// Rebuilds `logic`'s view under `theme`, lays it out in `window` with
    /// `faces` registered, paints it through a real `RenderRoot`, and
    /// returns every painted glyph run's font bytes, in paint order.
    pub(crate) fn painted_run_faces<V: View<()>>(
        mut logic: impl FnMut(&mut ()) -> V,
        theme: Theme,
        faces: &[&'static [u8]],
        window: Size,
    ) -> Vec<Vec<u8>> {
        let mut tcx = TextContext::new();
        for face in faces {
            tcx.register_fonts(face.to_vec())
                .expect("a Cupertino typeface-probe test face must register");
        }
        let mut root: RenderRoot<(), V> = RenderRoot::new();
        root.set_theme(Box::new(theme));
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(window, &mut tcx as &mut dyn Any);
        let mut recorder = FaceRecorder::default();
        root.paint(&mut recorder, FrameTime::ZERO);
        recorder.runs
    }

    /// Asserts `logic`'s component, painted under `theme` with `faces`
    /// registered, painted at least one glyph run and every run shaped
    /// against `expected`'s exact bytes — after first asserting the control
    /// (mirroring `frust-material`'s `typeface_probe::assert_paints_only_in`):
    /// a plain `text(..)` (the `SystemUi` request an un-opted Cupertino text
    /// makes) painted the same way under `theme`/`faces` must have no run in
    /// `expected`, or the identity assertion below would prove nothing (a host
    /// whose `SystemUi` happened to resolve to the probe face).
    #[track_caller]
    pub(crate) fn assert_paints_only_in<V: View<()>>(
        what: &str,
        logic: impl FnMut(&mut ()) -> V,
        theme: Theme,
        faces: &[&'static [u8]],
        window: Size,
        expected: &'static [u8],
    ) {
        let control = painted_run_faces(|_: &mut ()| text("Hello"), theme.clone(), faces, window);
        let leaked = control
            .iter()
            .filter(|bytes| bytes.as_slice() == expected)
            .count();
        assert_eq!(
            leaked,
            0,
            "control: {leaked} of {} run(s) of a plain text() that never opts in \
             shaped in the probe's registered face — on this host an un-opted run \
             is indistinguishable from the expected face, so the identity \
             assertion for {what} would prove nothing",
            control.len()
        );

        let runs = painted_run_faces(logic, theme, faces, window);
        assert!(!runs.is_empty(), "{what} painted no glyph run at all");
        let foreign = runs
            .iter()
            .filter(|bytes| bytes.as_slice() != expected)
            .count();
        assert_eq!(
            foreign,
            0,
            "{foreign} of {} glyph run(s) in {what} shaped against a face other \
             than the probe's registered face — the text is not asking for the \
             theme's type-scale family",
            runs.len()
        );
    }
}
