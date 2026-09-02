//! The assembled beUI [`Theme`]: the vendored token tables folded onto frust's
//! fixed token scales, plus [`BeuiTokens`] — the typed extension carrying every
//! beUI token that has no role to live in.
//!
//! [`theme()`](fn@theme) is the entry point ([`install`](crate::install) seeds
//! it).
//!
//! # How the theme is composed
//!
//! Through [`ThemeBuilder`] over [`Theme::neutral`], the way a third-party
//! design system is meant to compose one (the `sample-design` precedent, which
//! `frust-shadcn` follows too), never by filling a `Theme` literal field by
//! field:
//!
//! 1. baseline: [`Theme::neutral`] — the design-language-free floor, so every
//!    token this module does not set is a deliberate "no opinion" rather than an
//!    inherited Material one.
//! 2. `colors_light`/`colors_dark`: [`color_scheme`]'s 46-role mapping.
//! 3. `type_scale`: [`type_scale`] — the bundled Geist family over the neutral
//!    numbers.
//! 4. `shape`: [`shape_scale`] — the Tailwind radius ladder beUI resolves
//!    against.
//! 5. `glass`: [`glass_scale`] — beUI's three real translucent panel recipes.
//! 6. `design_language`: [`DesignLanguage::Custom`]`(`[`BEUI_DESIGN_LANGUAGE`]`)`.
//! 7. `extension`: [`BeuiTokens`] (ring/glass/gradients/brand hues) and
//!    [`NativeTypefaces`] (the bundled faces, for native controls).
//!
//! `Elevation` and `MotionScheme` stay at the neutral floor. beUI's shadows are
//! Tailwind's flat CSS ladder picked per component rather than levels derived
//! from a dp value, so they live in [`crate::style`] as constants; its motion
//! vocabulary is curves and springs rather than the duration/easing pairs
//! `MotionScheme` models, and it lives in [`super::motion`].
//!
//! **Brightness is deliberately not pinned.** The builder leaves
//! `Theme::neutral`'s own value in place; a shell re-derives light/dark from the
//! platform against whatever base [`install`](crate::install) seeded, and
//! pinning it here would fight that. It matters more for this catalog than for
//! most: beUI's own dark mode is a `.dark` class variant the user toggles.
//!
//! # Glass is a real `GlassScale` here, not extension data
//!
//! beUI has genuine glass chrome — three `backdrop-filter` recipes
//! (`.glass` / `.glass-strong` / `.glass-thin`) with real blur radii and
//! translucent washes — and frust models exactly that with `GlassScale`, whose
//! own docs direct a design system with real glass to author one from its mined
//! source values and install it through `ThemeBuilder::glass`. So the recipes
//! ride the theme's `glass` slot ([`glass_scale`]) where a widget already knows
//! to look, and [`BeuiTokens`] carries the raw `--glass-*` wash *colors*
//! alongside for a component that wants to name the CSS token directly. The two
//! are not two sources of truth: [`glass_scale`] is derived from the same
//! [`BeuiGlass`] tables the extension exposes.
//!
//! # Status colors stay at the neutral floor
//!
//! beUI authors two status hues (`--success`, `--warning`) and no `info` at all,
//! while `StatusColors` wants twelve values (a base, an ink, a container and a
//! container ink for each of success/warning/info). Filling that from two
//! authored colors would invent ten values upstream never wrote, so this theme
//! leaves `Theme::neutral`'s `StatusPalette` in place and publishes beUI's two
//! hues as what they are — brand colors — on [`BeuiTokens`].
//!
//! # The 46-role mapping, and what it had to decide
//!
//! beUI authors ~20 color roles; `ColorScheme` has 46. [`color_scheme`] holds
//! the whole mapping, each judgment call commented at the field it decides. The
//! ones worth knowing before reading it:
//!
//! - **`primary` is the ink color.** beUI aliases `--primary: var(--foreground)`
//!   and `--primary-foreground: var(--background)`, so its default button is
//!   near-black on light and near-white on dark. That maps straight across; it
//!   is not a mistake in the transcription.
//! - **The cyan `--accent` becomes the container pair.**
//!   `primary_container`/`on_primary_container` take `accent`/`accent-foreground`
//!   — the one saturated fill beUI paints for a highlighted state. `tertiary`
//!   takes the same pair, since beUI names no third accent role.
//! - **`--border-strong` is `outline`, `--border` is `outline_variant`.** beUI
//!   authors two hairline weights and M3's two outline roles are exactly a
//!   stronger and a subtler rule, so the ladder folds without duplication. The
//!   focus ring is `--border-strong` again (upstream aliases `--ring` onto it),
//!   but it rides [`BeuiTokens`] rather than a `ColorScheme` role, because
//!   `ColorScheme` has no focus-ring role and `surface_tint` is spoken for by
//!   the M3 `surface_tint == primary` convention every theme in the tree keeps.
//! - **`--danger` has no paired foreground token.** beUI paints `text-white`
//!   over `bg-destructive`, so `on_error` is white rather than a token read.
//!   `error_container` is `destructive` at 10% alpha — beUI's own
//!   `bg-destructive/10` — kept as alpha rather than flattened, so it
//!   composites over whatever surface it lands on.
//! - **The surface ladder has only two rungs upstream.** beUI authors
//!   `--background` and `--card` and aliases `--popover`/`--muted`/`--secondary`
//!   onto `--card`, so the five `surface_container*` roles collapse onto those
//!   two rather than inventing three intermediate greys.
//! - **The 12 `*_fixed*` roles are filled from the light table in both
//!   schemes.** That is what "fixed" means in M3 (brightness-invariant), and it
//!   keeps a widget that paints one from inheriting the neutral floor's
//!   slate-blue accent, which would be the only non-beUI hue in the theme. beUI
//!   has no dim variant, so `*_fixed_dim` repeats `*_fixed`.
//! - **`surface_dim`/`surface_bright` swap by brightness.** In light mode the
//!   dimmest surface is `card` and the brightest is `background`; in dark mode
//!   it is the other way round. Assigning one order to both brightnesses would
//!   make "dim" mean "lighter than the surface" in one of them.
//! - **`inverse_primary` is the *other* brightness's `primary`** — an inverse
//!   surface is the other brightness's surface, so the accent that reads on it
//!   is the other brightness's accent.

use frust::{
    Brightness, Color, ColorScheme, DesignLanguage, FontFace, GlassFill, GlassMaterial, GlassScale,
    NativeTypefaces, ShadowSpec, ShapeScale, Theme, ThemeBuilder, TypeScale,
    authoring::text::{FontFamily, GenericSlot, TextStyle},
};

use super::fonts::{self, GEIST_VARIABLE_INDEX};
use super::palette::{BEUI_DARK, BEUI_LIGHT, BeuiGlass, BeuiGradient, BeuiPalette};

/// This design system's stable identity tag, carried on
/// [`Theme::design_language`] as [`DesignLanguage::Custom`].
///
/// A host or widget that branches on design language sees this id rather than
/// mistaking the system for Material; every built-in `==` branch site treats an
/// unrecognized `Custom` tag as the neutral/System path, which is what a
/// web-derived system with no platform-native counterpart wants.
pub const BEUI_DESIGN_LANGUAGE: &str = "beui";

/// The family name Geist's own `name` table reports (`name` ID 1) — what a font
/// stack must name for the bundled face to resolve.
pub const GEIST_FAMILY: &str = "Geist";

/// The family name Geist Mono's own `name` table reports (`name` ID 1).
pub const GEIST_MONO_FAMILY: &str = "Geist Mono";

/// The unthemed fallback ring color: light mode's `--ring`
/// (`oklch(15% 0 0 / 0.12)`).
///
/// The bottom rung of [`BeuiTokens::resolve_ring`]'s ladder — used when no theme
/// is threaded into the pass *and* no explicit value was given.
pub const FALLBACK_RING: Color = BEUI_LIGHT.ring;

/// Alpha of the `destructive` wash `error_container` carries — beUI's
/// `bg-destructive/10`.
const DESTRUCTIVE_WASH_ALPHA: f32 = 0.10;

/// Alpha of the modal scrim — beUI's `bg-black/40` (`drawer.tsx` and
/// `animated-sidebar.tsx` both use it verbatim, and they are the only two
/// full-screen overlays upstream ships).
const SCRIM_ALPHA: f32 = 0.40;

/// Return `color` with its alpha channel replaced by `alpha` (the same
/// per-module helper shape `frust-widgets` and the other catalogs carry).
const fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

// ---- Type ------------------------------------------------------------------

/// The sans font stack: the bundled Geist face, then the platform's generic
/// sans — so text still renders (in the system sans) if a host never drained the
/// font registry.
///
/// This is upstream's `--font-sans` **and** its `--font-display`: beUI binds
/// both to Geist, so there is no separate display stack to model.
pub fn sans_family() -> FontFamily {
    FontFamily::stack_with_generic([GEIST_FAMILY], GenericSlot::SansSerif)
}

/// The monospace font stack: the bundled Geist Mono face, then the platform's
/// generic monospace — upstream's `--font-mono`.
///
/// `TypeScale` has no monospace slot, so this is the seam a component that needs
/// mono text (a code block, a key cap, tabular figures) builds its own
/// `TextStyle` from, the same way it would name `font-mono` in a class list.
pub fn mono_family() -> FontFamily {
    FontFamily::stack_with_generic([GEIST_MONO_FAMILY], GenericSlot::Monospace)
}

/// The beUI type scale: [`TypeScale::neutral`]'s numeric scale with every slot's
/// family swapped to [`sans_family`].
///
/// **Family-only.** beUI sizes type per component from Tailwind's `text-xs` /
/// `text-sm` / `text-base` steps rather than from a document-wide scale, so
/// those sizes live in [`crate::style`] where a component reads them, and this
/// scale exists to do one job: make the bundled Geist face the theme's actual
/// text family, for baseline widgets and beUI components alike. Re-deriving the
/// 30 M3 slots into a beUI vocabulary they have no counterpart for would invent
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

// ---- Shape -----------------------------------------------------------------

/// The beUI shape scale: the Tailwind radius ladder
/// ([`crate::style`]'s `RADIUS_*` constants) spread across `ShapeScale`'s ten
/// slots.
///
/// beUI overrides no radius token of its own — it has no `--radius` custom
/// property at all — so the ladder is Tailwind's own default scale, and
/// [`crate::style`] is where it is transcribed with its `rem` sources.
///
/// Tailwind's named ladder has seven steps and `ShapeScale` has nine finite
/// ones, so the top slot clamps to `4xl` (the Glyph precedent for a shorter
/// source scale) rather than extrapolating an eighth step upstream never
/// authors. `full` stays the pill sentinel every consumer resolves against a
/// box — and it is the step beUI's own controls reach for most (`rounded-full`
/// outnumbers every other radius class upstream, which is why
/// [`crate::style::RADIUS_CONTROL`] is that sentinel rather than a number off
/// this ladder).
pub const fn shape_scale() -> ShapeScale {
    ShapeScale {
        none: 0.0,
        extra_small: crate::style::RADIUS_SM,
        small: crate::style::RADIUS_MD,
        medium: crate::style::RADIUS_LG,
        large: crate::style::RADIUS_XL,
        large_increased: crate::style::RADIUS_2XL,
        extra_large: crate::style::RADIUS_3XL,
        extra_large_increased: crate::style::RADIUS_4XL,
        extra_extra_large: crate::style::RADIUS_4XL, // clamped: the ladder stops at 4xl
        full: crate::style::RADIUS_FULL,
    }
}

// ---- Glass -----------------------------------------------------------------

/// Blur radius of the `.glass` utility, in CSS px: `backdrop-filter: blur(20px)`.
const GLASS_BLUR_PX: f64 = 20.0;
/// Blur radius of the `.glass-strong` utility: `backdrop-filter: blur(16px)`.
const GLASS_STRONG_BLUR_PX: f64 = 16.0;
/// Blur radius of the `.glass-thin` utility: `backdrop-filter: blur(12px)`.
const GLASS_THIN_BLUR_PX: f64 = 12.0;

/// Alpha of the specular hairline the glass tiers stroke, taken from dark
/// mode's `--glass-border` (`rgb(255 255 255 / 0.08)`).
///
/// `GlassMaterial::hairline_alpha` is a single scalar for a *white* edge, while
/// beUI authors `--glass-border` as a color that inverts by brightness (a black
/// 8% wash in light mode, a white 8% wash in dark). Only the dark-mode reading
/// fits the field, so that is what it carries; the light-mode hairline color is
/// preserved verbatim on [`BeuiTokens::glass`] for a component that strokes the
/// edge itself. The alpha is the same 0.08 in both, so nothing but the hue is
/// lost here.
const GLASS_HAIRLINE_ALPHA: f32 = 0.08;

/// beUI's glass recipes, folded onto frust's three-tier `GlassScale`.
///
/// **Source:** the `.glass`, `.glass-strong` and `.glass-thin` utility classes
/// in `app/globals.css` (same rev as the palette).
///
/// The tier assignment goes by *role*, not by opacity:
///
/// - `chrome` (popovers/sheets/menus) ← `.glass`, the tier upstream decorates as
///   a floating panel: the highest blur (20px), the only hairline-plus-shadow
///   treatment, and the wash `--glass-bg`.
/// - `bar` (tab/nav/toolbars) ← `.glass-strong` (16px).
/// - `control` (buttons/toggles) ← `.glass-thin`, the sheerest wash (12px).
///
/// Note this deviates from `GlassScale`'s own field doc, which describes
/// `chrome` as *"the most opaque, highest-blur tier"*. In beUI the two halves of
/// that description disagree: `.glass` carries the highest blur but a 0.55 wash,
/// while `.glass-strong` carries a 0.7 wash at 16px blur. Role won over opacity
/// because a widget picks a tier by what it *is* (a panel, a bar, a control),
/// and `.glass` is unambiguously upstream's panel treatment — it is the only one
/// with a drop shadow.
///
/// # What does not survive the fold
///
/// - **`saturate(160%)`**, which `.glass` pairs with its blur. `GlassMaterial`
///   models a blur intent and washes, not a backdrop color-matrix, and there is
///   no field to put it in. The consequence is a slightly less vivid backdrop
///   than the web original.
/// - **The inset top highlight** `.glass` also draws
///   (`0 1px 0 0 rgb(255 255 255 / 0.06) inset`). `ShadowSpec` is a drop shadow
///   (offset, blur, alpha) with no inset form, so this is left to the component
///   that paints the panel edge.
///
/// The blur radii convert from CSS to `blur_std_dev` by halving: a CSS blur
/// radius is twice the Gaussian standard deviation it names.
pub fn glass_scale() -> GlassScale {
    let fill = |color: Color| {
        let [r, g, b, a] = color.components;
        GlassFill::new(r, g, b, a)
    };
    let tier = |blur_px: f64, wash: fn(&BeuiGlass) -> Color, hairline: f32, shadow: ShadowSpec| {
        GlassMaterial {
            blur_radius_intent: blur_px / 2.0,
            fills_light: vec![fill(wash(&BEUI_LIGHT.glass))],
            fills_dark: vec![fill(wash(&BEUI_DARK.glass))],
            hairline_alpha: hairline,
            shadow,
        }
    };
    GlassScale {
        chrome: tier(
            GLASS_BLUR_PX,
            |g| g.bg,
            GLASS_HAIRLINE_ALPHA,
            // `.glass`'s drop shadow: `0 24px 60px -24px rgb(0 0 0 / 0.45)`.
            // The `-24px` spread has no `ShadowSpec` field and is dropped; it
            // pulls the shadow in horizontally, so this reads a touch wider
            // than the original.
            ShadowSpec {
                y_offset: 24.0,
                blur_std_dev: 30.0,
                color_alpha: 0.45,
            },
        ),
        // `.glass-strong` declares neither a border nor a shadow upstream — it
        // is a bare wash over a blur — so both stay zero rather than borrowing
        // the panel tier's.
        bar: tier(
            GLASS_STRONG_BLUR_PX,
            |g| g.strong_bg,
            0.0,
            ShadowSpec {
                y_offset: 0.0,
                blur_std_dev: 0.0,
                color_alpha: 0.0,
            },
        ),
        // `.glass-thin` strokes the same hairline `.glass` does, but declares no
        // shadow.
        control: tier(
            GLASS_THIN_BLUR_PX,
            |g| g.thin_bg,
            GLASS_HAIRLINE_ALPHA,
            ShadowSpec {
                y_offset: 0.0,
                blur_std_dev: 0.0,
                color_alpha: 0.0,
            },
        ),
    }
}

// ---- The typed extension ---------------------------------------------------

/// The beUI theme extension: the focus ring, the glass wash colors, the two
/// gradients, and the brand hues — every beUI token with no `ColorScheme` role
/// to live in.
///
/// Attached by [`theme()`](fn@theme); recovered by a widget with
/// `theme.extension::<BeuiTokens>()`. Each group is here for its own reason:
///
/// - **`--ring`** — a focus-ring color. `ColorScheme` has no focus-ring role,
///   and `surface_tint` is spoken for (see the [module docs](self)).
/// - **The `--glass-*` wash colors** — the recipes themselves ride the theme's
///   `GlassScale` ([`glass_scale`]); these are the raw token *colors*, for a
///   component that wants to name `--glass-border` directly rather than
///   reconstruct it from a hairline alpha.
/// - **`--gradient-bg` / `--gradient-accent`** — no gradient role exists, and
///   no ambition to invent one.
/// - **`--neon` / `--violet` / `--success` / `--warning`** — brand hues.
///   `success`/`warning` are *not* folded into `StatusPalette`: beUI authors two
///   colors where that type wants twelve (see the [module docs](self)).
///
/// Everything is per-brightness where the source differs and
/// brightness-invariant where it does not — a beUI theme carries **both**
/// brightnesses' values at once, exactly like `Theme::light`/`Theme::dark`,
/// because a shell may flip brightness at any time without rebuilding the theme.
///
/// A `Theme` is cloned across the framework's two delivery paths, so an
/// extension must be `Any + Send + Sync` — this is plain `Copy` data and
/// satisfies that for free.
///
/// # Resolution precedence
///
/// Consumers resolve through the framework's documented ladder — **explicit
/// builder value > theme extension > fallback constant** — via
/// [`BeuiTokens::resolve_ring`] and [`BeuiTokens::resolve`], the two places that
/// ladder is written down for this catalog. A theme with the extension cleared
/// — an app is free to build one — falls through to the fallback rather than
/// panicking or painting nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeuiTokens {
    /// `--ring`, light mode.
    pub ring_light: Color,
    /// `--ring`, dark mode.
    pub ring_dark: Color,
    /// The `--glass-*` group, light mode.
    pub glass_light: BeuiGlass,
    /// The `--glass-*` group, dark mode.
    pub glass_dark: BeuiGlass,
    /// `--gradient-bg`, light mode.
    pub gradient_bg_light: BeuiGradient,
    /// `--gradient-bg`, dark mode.
    pub gradient_bg_dark: BeuiGradient,
    /// `--gradient-accent`, light mode.
    pub gradient_accent_light: BeuiGradient,
    /// `--gradient-accent`, dark mode.
    pub gradient_accent_dark: BeuiGradient,
    /// `--neon`: a decorative brand green (brightness-invariant upstream).
    pub neon: Color,
    /// `--violet`: a decorative brand violet (brightness-invariant upstream).
    pub violet: Color,
    /// `--success`: the success status hue (brightness-invariant upstream).
    pub success: Color,
    /// `--warning`: the warning status hue (brightness-invariant upstream).
    pub warning: Color,
}

impl BeuiTokens {
    /// The extension [`theme()`](fn@theme) attaches.
    pub const fn beui() -> Self {
        Self {
            ring_light: BEUI_LIGHT.ring,
            ring_dark: BEUI_DARK.ring,
            glass_light: BEUI_LIGHT.glass,
            glass_dark: BEUI_DARK.glass,
            gradient_bg_light: BEUI_LIGHT.gradient_bg,
            gradient_bg_dark: BEUI_DARK.gradient_bg,
            gradient_accent_light: BEUI_LIGHT.gradient_accent,
            gradient_accent_dark: BEUI_DARK.gradient_accent,
            // Authored once in `:root` and inherited by `.dark`, so the light
            // table is the only source there is.
            neon: BEUI_LIGHT.neon,
            violet: BEUI_LIGHT.violet,
            success: BEUI_LIGHT.success,
            warning: BEUI_LIGHT.warning,
        }
    }

    /// The ring color for `brightness`.
    pub fn ring(&self, brightness: Brightness) -> Color {
        match brightness {
            Brightness::Light => self.ring_light,
            Brightness::Dark => self.ring_dark,
        }
    }

    /// The glass wash colors for `brightness`.
    pub fn glass(&self, brightness: Brightness) -> BeuiGlass {
        match brightness {
            Brightness::Light => self.glass_light,
            Brightness::Dark => self.glass_dark,
        }
    }

    /// The ambient background gradient for `brightness`.
    pub fn gradient_bg(&self, brightness: Brightness) -> BeuiGradient {
        match brightness {
            Brightness::Light => self.gradient_bg_light,
            Brightness::Dark => self.gradient_bg_dark,
        }
    }

    /// The brand accent gradient for `brightness`.
    pub fn gradient_accent(&self, brightness: Brightness) -> BeuiGradient {
        match brightness {
            Brightness::Light => self.gradient_accent_light,
            Brightness::Dark => self.gradient_accent_dark,
        }
    }

    /// Recover the extension from a theme, falling back to
    /// [`BeuiTokens::beui`] when the theme carries none.
    ///
    /// The second rung of the precedence ladder, for every token group that has
    /// no per-value explicit override — a component reads the group and picks
    /// the field it wants.
    pub fn resolve(theme: Option<&Theme>) -> Self {
        theme
            .and_then(|t| t.extension::<BeuiTokens>())
            .copied()
            .unwrap_or_else(Self::beui)
    }

    /// Resolve a focus-ring color under the framework's documented precedence:
    /// **explicit builder value > theme extension > fallback constant**.
    ///
    /// This and [`resolve`](Self::resolve) are the only places the ladder is
    /// spelled out for this catalog; every component that paints a focus ring
    /// calls through here, so two components can never disagree about
    /// precedence.
    ///
    /// The theme rung reads the extension **and** the theme's own brightness, so
    /// a ring follows a live light/dark flip with no component involvement.
    pub fn resolve_ring(explicit: Option<Color>, theme: Option<&Theme>) -> Color {
        if let Some(color) = explicit {
            return color;
        }
        match theme.and_then(|t| t.extension::<BeuiTokens>().map(|x| (x, t.brightness))) {
            Some((tokens, brightness)) => tokens.ring(brightness),
            None => FALLBACK_RING,
        }
    }
}

// ---- The color-scheme fold -------------------------------------------------

/// Fold beUI's two token tables onto frust's 46-role `ColorScheme` for one
/// `brightness`.
///
/// Both tables are taken because two role groups need the one this brightness is
/// *not*: the 12 `*_fixed*` roles (always the light table, since "fixed" means
/// brightness-invariant) and `inverse_primary` (the other brightness's accent).
/// See the [module docs](self) for the reasoning behind each judgment call; the
/// per-field comments below name which decision each line is.
pub fn color_scheme(
    light: &BeuiPalette,
    dark: &BeuiPalette,
    brightness: Brightness,
) -> ColorScheme {
    let (p, other) = match brightness {
        Brightness::Light => (light, dark),
        Brightness::Dark => (dark, light),
    };
    // The dimmest/brightest surface swap ends: light mode dims toward `card`,
    // dark mode brightens toward it.
    let (dim, bright) = match brightness {
        Brightness::Light => (p.card, p.background),
        Brightness::Dark => (p.background, p.card),
    };

    ColorScheme {
        // beUI's `--primary` is the ink color and its `--accent` the one
        // saturated fill, so the accent stands in for M3's container pair.
        primary: p.primary,
        on_primary: p.primary_foreground,
        primary_container: p.accent,
        on_primary_container: p.accent_foreground,
        primary_fixed: light.primary,
        primary_fixed_dim: light.primary, // no dim variant upstream
        on_primary_fixed: light.primary_foreground,
        on_primary_fixed_variant: light.muted_foreground,

        // `secondary` maps straight across; its container repeats it, since
        // beUI aliases `--secondary` onto `--card` and draws no distinction
        // between a secondary fill and a secondary container.
        secondary: p.secondary,
        on_secondary: p.secondary_foreground,
        secondary_container: p.secondary,
        on_secondary_container: p.secondary_foreground,
        secondary_fixed: light.secondary,
        secondary_fixed_dim: light.secondary,
        on_secondary_fixed: light.secondary_foreground,
        on_secondary_fixed_variant: light.muted_foreground,

        // beUI names no third accent role, so `tertiary` takes the cyan
        // `accent` — the nearest thing to one it authors. (`--violet`, its
        // other brand hue, is decorative and rides `BeuiTokens` instead: it has
        // no paired ink token, so it cannot fill an on-color role here.)
        tertiary: p.accent,
        on_tertiary: p.accent_foreground,
        tertiary_container: p.accent,
        on_tertiary_container: p.accent_foreground,
        tertiary_fixed: light.accent,
        tertiary_fixed_dim: light.accent,
        on_tertiary_fixed: light.accent_foreground,
        on_tertiary_fixed_variant: light.muted_foreground,

        // `--destructive` (an alias of `--danger`) → the error family. White ink
        // is beUI's own choice (`bg-destructive text-white`), not a token read;
        // the container is its 10% wash, kept translucent.
        error: p.destructive,
        on_error: Color::WHITE,
        error_container: with_alpha(p.destructive, DESTRUCTIVE_WASH_ALPHA),
        on_error_container: p.destructive,

        // The surface ladder has two authored rungs — `background` and `card`
        // (which `--popover`/`--muted`/`--secondary` all alias) — so the five
        // container roles collapse onto them instead of inventing greys
        // upstream never wrote.
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

        // beUI's two authored hairline weights fold onto M3's two outline
        // roles: `--border-strong` is the emphasized rule, `--border` the
        // everyday one.
        outline: p.border_strong,
        outline_variant: p.border,
        // beUI's shadows are `rgb(0 0 0 / α)`; the alpha travels with each
        // shadow constant in `crate::style`, so the role itself is plain black.
        shadow: Color::BLACK,
        scrim: with_alpha(Color::BLACK, SCRIM_ALPHA),
        // An inverse surface is the other brightness's surface — and the accent
        // that reads on it is that brightness's accent.
        inverse_surface: p.foreground,
        inverse_on_surface: p.background,
        inverse_primary: other.primary,
        // Keeps the M3 `surface_tint == primary` convention; the focus ring
        // lives in `BeuiTokens`, not here.
        surface_tint: p.primary,
    }
}

/// beUI's light-mode `ColorScheme` — the flat-named constructor the external
/// design-system contract asks for.
pub fn color_scheme_light() -> ColorScheme {
    color_scheme(&BEUI_LIGHT, &BEUI_DARK, Brightness::Light)
}

/// beUI's dark-mode `ColorScheme`.
pub fn color_scheme_dark() -> ColorScheme {
    color_scheme(&BEUI_LIGHT, &BEUI_DARK, Brightness::Dark)
}

/// The native-control typeface binding [`theme()`](fn@theme) attaches: the
/// bundled Geist face in both slots.
///
/// beUI names one sans family for controls and body text alike (its
/// `--font-display` is Geist too), so unlike Glyph's display/body split both
/// slots carry the same face. Both are taken **out of [`fonts::font_data`]'s own
/// array** rather than re-referenced from the underlying constants: a native
/// host de-duplicates published payloads by byte identity (address + length), so
/// a slot's face and the bytes a shell registers through
/// [`install`](crate::install) must be the *same* `&'static [u8]`, not merely
/// equal ones.
pub fn native_typefaces() -> NativeTypefaces {
    match fonts::font_data().get(GEIST_VARIABLE_INDEX).copied() {
        Some(bytes) => NativeTypefaces::uniform(FontFace::new(GEIST_FAMILY, bytes)),
        None => NativeTypefaces::default(),
    }
}

/// The beUI theme. See the [module docs](self) for the composition order and the
/// mapping decisions.
///
/// This is what [`install`](crate::install) seeds as the app's default theme.
pub fn theme() -> Theme {
    ThemeBuilder::new(Theme::neutral())
        .colors_light(color_scheme_light())
        .colors_dark(color_scheme_dark())
        .type_scale(type_scale(&TextStyle::default()))
        .shape(shape_scale())
        .glass(glass_scale())
        .design_language(DesignLanguage::Custom(BEUI_DESIGN_LANGUAGE))
        .extension(BeuiTokens::beui())
        .extension(native_typefaces())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::StatusPalette;

    #[test]
    fn theme_carries_the_custom_design_language_tag() {
        let t = theme();
        assert_eq!(
            t.design_language,
            DesignLanguage::Custom(BEUI_DESIGN_LANGUAGE)
        );
        // Never mistakeable for a built-in language, or for the sibling
        // external-origin catalog.
        assert_ne!(t.design_language, DesignLanguage::Material3);
        assert_ne!(t.design_language, DesignLanguage::Cupertino);
        assert_ne!(t.design_language, DesignLanguage::Glyph);
        assert_ne!(t.design_language, DesignLanguage::Custom("shadcn"));
    }

    #[test]
    fn theme_is_internally_consistent_and_round_trips_its_parts() {
        let t = theme();
        assert_eq!(t.light, color_scheme_light());
        assert_eq!(t.dark, color_scheme_dark());
        assert_eq!(t.shape, shape_scale());
        assert_eq!(t.glass, glass_scale());
        assert_eq!(t.type_scale, type_scale(&TextStyle::default()));
        assert_eq!(t.extension::<BeuiTokens>(), Some(&BeuiTokens::beui()));
        assert_eq!(t.extension::<NativeTypefaces>(), Some(&native_typefaces()));
        // Survives the clone the framework's two delivery paths take.
        let c = t.clone();
        assert_eq!(c.extension::<BeuiTokens>(), Some(&BeuiTokens::beui()));
        assert_eq!(c.light, t.light);
    }

    #[test]
    fn theme_does_not_pin_brightness() {
        // A shell re-derives light/dark from the platform against the seeded
        // base; the builder must leave the neutral floor's own value alone.
        assert_eq!(theme().brightness, Theme::neutral().brightness);
        let dark = theme().with_brightness(Brightness::Dark);
        assert_eq!(dark.scheme(), &dark.dark);
    }

    /// The status palette is left at the neutral floor on purpose (see the
    /// module docs) — beUI's two authored status hues cannot fill twelve roles.
    #[test]
    fn status_colors_stay_at_the_neutral_floor() {
        let t = theme();
        assert_eq!(
            t.extension::<StatusPalette>(),
            Theme::neutral().extension::<StatusPalette>()
        );
        // ...and beUI's own two hues are reachable as brand colors instead.
        let tokens = BeuiTokens::beui();
        assert_eq!(tokens.success, BEUI_LIGHT.success);
        assert_eq!(tokens.warning, BEUI_LIGHT.warning);
    }

    /// Both schemes resolve every role from a beUI token, with no neutral-floor
    /// value showing through. The check that would catch a forgotten field is
    /// the one below it (`ne` against the floor's own scheme); this one pins
    /// the roles a reader most wants to look up.
    #[test]
    fn both_schemes_resolve_from_the_beui_tables() {
        for (brightness, p) in [
            (Brightness::Light, BEUI_LIGHT),
            (Brightness::Dark, BEUI_DARK),
        ] {
            let s = color_scheme(&BEUI_LIGHT, &BEUI_DARK, brightness);
            assert_eq!(s.primary, p.foreground, "primary is the ink color");
            assert_eq!(s.on_primary, p.background);
            assert_eq!(s.primary_container, p.accent);
            assert_eq!(s.surface, p.background);
            assert_eq!(s.on_surface, p.foreground);
            assert_eq!(s.surface_container, p.card);
            assert_eq!(s.on_surface_variant, p.muted_foreground);
            assert_eq!(s.error, p.danger);
            assert_eq!(s.on_error, Color::WHITE);
            assert_eq!(s.outline, p.border_strong, "the emphasized rule");
            assert_eq!(s.outline_variant, p.border, "the everyday rule");
            assert_eq!(s.surface_tint, s.primary, "the M3 convention");
        }
        // Neither scheme is the framework's own floor.
        assert_ne!(color_scheme_light(), Theme::neutral().light);
        assert_ne!(color_scheme_dark(), Theme::neutral().dark);
        assert_ne!(color_scheme_light(), color_scheme_dark());
    }

    /// The three roles that deliberately read the *other* table: `*_fixed*` is
    /// brightness-invariant, and `inverse_primary` is the opposite scheme's
    /// accent.
    #[test]
    fn the_cross_brightness_roles_read_the_other_table() {
        let (l, d) = (color_scheme_light(), color_scheme_dark());
        for s in [&l, &d] {
            assert_eq!(s.primary_fixed, BEUI_LIGHT.primary);
            assert_eq!(s.primary_fixed_dim, s.primary_fixed, "no dim variant");
            assert_eq!(s.tertiary_fixed, BEUI_LIGHT.accent);
            assert_eq!(s.secondary_fixed, BEUI_LIGHT.secondary);
        }
        assert_eq!(l.inverse_primary, BEUI_DARK.primary);
        assert_eq!(d.inverse_primary, BEUI_LIGHT.primary);
    }

    /// The dim/bright pair swaps ends with brightness, so "dim" never means
    /// "lighter than the surface".
    #[test]
    fn surface_dim_and_bright_swap_with_brightness() {
        let luma = |c: Color| {
            let [r, g, b, _] = c.components;
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        for s in [color_scheme_light(), color_scheme_dark()] {
            assert!(
                luma(s.surface_dim) < luma(s.surface_bright),
                "surface_dim must be the darker of the pair"
            );
        }
    }

    #[test]
    fn the_extension_selects_per_brightness_groups() {
        let tokens = BeuiTokens::beui();
        assert_eq!(tokens.ring(Brightness::Light), BEUI_LIGHT.ring);
        assert_eq!(tokens.ring(Brightness::Dark), BEUI_DARK.ring);
        assert_eq!(tokens.glass(Brightness::Light), BEUI_LIGHT.glass);
        assert_eq!(tokens.glass(Brightness::Dark), BEUI_DARK.glass);
        assert_eq!(tokens.gradient_bg(Brightness::Dark), BEUI_DARK.gradient_bg);
        assert_eq!(
            tokens.gradient_accent(Brightness::Light),
            BEUI_LIGHT.gradient_accent
        );
        // The two brightnesses really are different tables.
        assert_ne!(tokens.ring_light, tokens.ring_dark);
        assert_ne!(tokens.glass_light, tokens.glass_dark);
    }

    #[test]
    fn ring_precedence_is_explicit_then_extension_then_fallback() {
        let explicit = Color::from_rgb8(0x00, 0x00, 0xFF);
        let t = theme();

        // 1. explicit wins over everything.
        assert_eq!(BeuiTokens::resolve_ring(Some(explicit), Some(&t)), explicit);
        // 2. no explicit value: the theme extension wins over the fallback, at
        //    the theme's own brightness.
        assert_eq!(
            BeuiTokens::resolve_ring(None, Some(&t)),
            BeuiTokens::beui().ring(t.brightness)
        );
        // 3. no theme at all: the fallback constant.
        assert_eq!(BeuiTokens::resolve_ring(None, None), FALLBACK_RING);
    }

    #[test]
    fn ring_follows_the_theme_s_brightness() {
        let light = theme().with_brightness(Brightness::Light);
        let dark = theme().with_brightness(Brightness::Dark);
        assert_eq!(
            BeuiTokens::resolve_ring(None, Some(&light)),
            BeuiTokens::beui().ring_light
        );
        assert_eq!(
            BeuiTokens::resolve_ring(None, Some(&dark)),
            BeuiTokens::beui().ring_dark
        );
    }

    #[test]
    fn a_theme_with_the_extension_cleared_falls_through_to_the_fallbacks() {
        // An app is free to build a theme carrying no BeuiTokens; both
        // resolvers must degrade, not panic.
        let bare = Theme::neutral();
        assert!(bare.extension::<BeuiTokens>().is_none());
        assert_eq!(
            BeuiTokens::resolve_ring(None, Some(&bare)),
            FALLBACK_RING,
            "a non-beUI theme resolves the fallback ring"
        );
        assert_eq!(BeuiTokens::resolve(Some(&bare)), BeuiTokens::beui());
        assert_eq!(BeuiTokens::resolve(None), BeuiTokens::beui());
        assert_eq!(BeuiTokens::resolve(Some(&theme())), BeuiTokens::beui());
    }

    /// beUI has real glass, so the theme's own `GlassScale` must be a lens in
    /// every tier — not the opaque floor `Theme::neutral` carries, which would
    /// make every glass-aware widget paint a solid surface instead.
    #[test]
    fn the_glass_scale_is_a_real_lens_in_every_tier() {
        let glass = glass_scale();
        for (name, tier) in [
            ("chrome", &glass.chrome),
            ("bar", &glass.bar),
            ("control", &glass.control),
        ] {
            assert!(!tier.is_opaque(), "{name} must carry blur intent");
            assert_eq!(tier.fills_light.len(), 1, "{name} light wash");
            assert_eq!(tier.fills_dark.len(), 1, "{name} dark wash");
            // Every wash is translucent — an opaque one would defeat the blur.
            for fill in tier.fills_light.iter().chain(&tier.fills_dark) {
                let alpha = fill.color.components[3];
                assert!(alpha > 0.0 && alpha < 1.0, "{name} wash alpha {alpha}");
            }
        }
        assert_ne!(glass, GlassScale::opaque_material());
        assert_eq!(theme().glass, glass);
    }

    /// The tiers carry the source's own blur ordering (20 / 16 / 12 CSS px,
    /// halved to standard deviations) and only the panel tier gets a shadow.
    #[test]
    fn the_glass_tiers_carry_their_source_blur_and_shadow() {
        let glass = glass_scale();
        assert_eq!(glass.chrome.blur_radius_intent, GLASS_BLUR_PX / 2.0);
        assert_eq!(glass.bar.blur_radius_intent, GLASS_STRONG_BLUR_PX / 2.0);
        assert_eq!(glass.control.blur_radius_intent, GLASS_THIN_BLUR_PX / 2.0);
        assert!(glass.chrome.blur_radius_intent > glass.bar.blur_radius_intent);
        assert!(glass.bar.blur_radius_intent > glass.control.blur_radius_intent);

        assert_eq!(glass.chrome.shadow.color_alpha, 0.45);
        assert_eq!(glass.bar.shadow.color_alpha, 0.0, "no shadow upstream");
        assert_eq!(glass.control.shadow.color_alpha, 0.0, "no shadow upstream");

        // The hairline follows the source: `.glass` and `.glass-thin` stroke
        // one, `.glass-strong` does not.
        assert_eq!(glass.chrome.hairline_alpha, GLASS_HAIRLINE_ALPHA);
        assert_eq!(glass.control.hairline_alpha, GLASS_HAIRLINE_ALPHA);
        assert_eq!(glass.bar.hairline_alpha, 0.0);
    }

    /// The glass recipes are derived from the same tables the extension
    /// publishes — the module docs' "not two sources of truth" claim, pinned.
    #[test]
    fn the_glass_scale_is_derived_from_the_palette_tables() {
        let glass = glass_scale();
        let tokens = BeuiTokens::beui();
        let wash = |fill: &GlassFill| fill.color;
        assert_eq!(wash(&glass.chrome.fills_light[0]), tokens.glass_light.bg);
        assert_eq!(wash(&glass.chrome.fills_dark[0]), tokens.glass_dark.bg);
        assert_eq!(
            wash(&glass.bar.fills_light[0]),
            tokens.glass_light.strong_bg
        );
        assert_eq!(
            wash(&glass.control.fills_dark[0]),
            tokens.glass_dark.thin_bg
        );
    }

    #[test]
    fn the_shape_scale_is_the_tailwind_ladder_and_ends_in_the_pill_sentinel() {
        let s = shape_scale();
        assert_eq!(s.none, 0.0);
        assert_eq!(s.extra_small, crate::style::RADIUS_SM);
        assert_eq!(s.medium, crate::style::RADIUS_LG);
        assert_eq!(s.extra_large_increased, crate::style::RADIUS_4XL);
        assert_eq!(
            s.extra_extra_large, s.extra_large_increased,
            "clamped: the source ladder stops at 4xl"
        );
        assert!(s.full.is_infinite(), "the pill sentinel");
        // Monotonic, so a component picking a "bigger" step always gets one.
        let steps = [
            s.none,
            s.extra_small,
            s.small,
            s.medium,
            s.large,
            s.large_increased,
            s.extra_large,
            s.extra_large_increased,
        ];
        assert!(steps.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn both_font_stacks_name_the_bundled_families_first() {
        // The families the bundled faces actually report; a rename here would
        // silently fall the whole catalog back to the system sans.
        assert_eq!(GEIST_FAMILY, "Geist");
        assert_eq!(GEIST_MONO_FAMILY, "Geist Mono");
        assert_ne!(sans_family(), mono_family());
        // The type scale really did take the sans stack everywhere.
        let scale = type_scale(&TextStyle::default());
        assert_eq!(scale.body_medium.family, sans_family());
        assert_eq!(scale.display_large.family, sans_family());
        assert_eq!(scale.label_small_emphasized.family, sans_family());
    }

    #[test]
    fn native_typefaces_bind_the_same_bytes_font_data_hands_a_shell() {
        // The byte-identity contract a native host's publish guard needs.
        let faces = native_typefaces();
        let bundled = fonts::font_data()[GEIST_VARIABLE_INDEX];
        assert_eq!(faces, native_typefaces());
        assert!(
            std::ptr::eq(bundled, fonts::font_data()[GEIST_VARIABLE_INDEX]),
            "the bound face must be the same `&'static [u8]` every call"
        );
    }
}
