//! Glyph color tokens mapped onto the 46-role [`ColorScheme`], for both
//! brightnesses, plus the brightness-invariant [`GlyphInk`] terminal/tooltip
//! tokens and the Glyph [`StatusPalette`].
//!
//! # Sources
//!
//! - **Dark** ([`ColorScheme::glyph_dark`]): `glyph-design-system.html`'s
//!   `:root` CSS custom properties (the canonical brightness), retrieved
//!   2026-07-21 (RESEARCH §1.1).
//! - **Light** ([`ColorScheme::glyph_light`]): `glyph-design-system-light.html`'s
//!   `:root` block, retrieved 2026-07-21 (RESEARCH §1.1b). Not a value flip —
//!   the accent, semantic hues, and surface ordering are independently
//!   re-tuned for AA on paper.
//! - **Ink** ([`GlyphInk`]) and **status** ([`StatusPalette::glyph`]) sources
//!   are cited on those items.
//!
//! # Accent-role convention (uniform across brightnesses; PLAN.md Phase 3)
//!
//! Glyph is a **single-accent** system whose accent plays two roles the way
//! Material 3 splits `primary` (text/icon) from `primary_container` (a filled
//! chip/button). Light mode forces the split apart — `#ffb627` fails AA as
//! text on paper but is AA-safe as a filled button with near-black text — so
//! the mapping fixes the convention **once, for both brightnesses**:
//!
//! - `primary` = the accent **text/icon** color (dark `#ffb627`, light
//!   `#a3650a`), with `on_primary` the ink a widget filling with `primary`
//!   would place on it.
//! - `primary_container` = the **bright fill** amber (`#ffb627`, both
//!   brightnesses), with `on_primary_container` the near-black amber ink
//!   (`#241a04` dark, `#2a1c04` light) the source pairs with a filled amber
//!   button.
//!
//! # Alpha pre-flattening
//!
//! Glyph authors faint washes and hairline borders as `rgba(...)` over a
//! surface. [`ColorScheme`] roles are opaque, so every alpha token that maps
//! to an opaque role is **pre-flattened over its canonical background** —
//! `bg-surface` (`#161a23` dark, `#ffffff` light) for card-level washes —
//! with the source alpha noted per field. (The genuinely-translucent
//! Cupertino roles are a different case; Glyph flattens because these washes
//! are always composited over a known surface.)
//!
//! # Nearest-role mappings (documented decisions)
//!
//! Glyph's flat token set is smaller than M3's 46 roles, so several roles have
//! no direct source token and are mapped to the nearest Glyph family with a
//! per-field comment: the neutral `secondary` family borrows the muted-fg /
//! raised-surface tones (Glyph's secondary buttons are `bg-raised`); the
//! `tertiary` family borrows the `cyan` info hue (Glyph's only spare
//! hue-bearing token — "semantic color is the only thing that shifts hue");
//! and the `*_fixed`/`inverse_*` M3-only concepts are Frust-authored
//! brightness-invariant derivations from the amber/neutral/cyan families, the
//! same approach [`ColorScheme::cupertino_light`] documents for iOS's own
//! missing M3 concepts. See the task's completion summary for the full
//! leftover list.

use peniko::Color;

use crate::color::ColorScheme;
use crate::status::{StatusColors, StatusPalette};

// ---- Glyph raw source tokens (both brightnesses) -----------------------

// Dark (`glyph-design-system.html` :root, retrieved 2026-07-21).
const D_BG_VOID: Color = Color::from_rgb8(0x0a, 0x0c, 0x11); // page
const D_BG_BASE: Color = Color::from_rgb8(0x10, 0x13, 0x1a); // demo/base surfaces
const D_BG_SURFACE: Color = Color::from_rgb8(0x16, 0x1a, 0x23); // cards, inputs
const D_BG_RAISED: Color = Color::from_rgb8(0x1e, 0x23, 0x30); // raised / secondary buttons
const D_BG_OVERLAY: Color = Color::from_rgb8(0x27, 0x2d, 0x3d); // overlays, toggles, track
const D_BG_HOVER: Color = Color::from_rgb8(0x2f, 0x36, 0x48); // hover fill
const D_AMBER: Color = Color::from_rgb8(0xff, 0xb6, 0x27); // THE accent
const D_AMBER_DIM: Color = Color::from_rgb8(0xc9, 0x8a, 0x12);
const D_CYAN: Color = Color::from_rgb8(0x5e, 0xc8, 0xd8); // info hue
const D_ERROR: Color = Color::from_rgb8(0xff, 0x6b, 0x6b);
const D_FG: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // primary text
const D_FG_MUTED: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // secondary text
// `fg-dim` (#6b6556 dark / #948a70 light — tertiary/label text) has no distinct
// `ColorScheme` role (on_surface_variant already carries `fg-muted`); it is a
// documented leftover (see the task completion summary), not mapped here.

// On-accent ink: near-black text on an amber fill (`.btn-primary` color).
const D_ON_AMBER: Color = Color::from_rgb8(0x24, 0x1a, 0x04);
const L_ON_AMBER: Color = Color::from_rgb8(0x2a, 0x1c, 0x04);

// Light (`glyph-design-system-light.html` :root, retrieved 2026-07-21).
const L_BG_VOID: Color = Color::from_rgb8(0xf6, 0xf2, 0xe9); // page
const L_BG_BASE: Color = Color::from_rgb8(0xfd, 0xfb, 0xf6);
const L_BG_SURFACE: Color = Color::from_rgb8(0xff, 0xff, 0xff); // cards/inputs — the LIGHTEST slot
const L_BG_RAISED: Color = Color::from_rgb8(0xf1, 0xec, 0xe0);
const L_BG_OVERLAY: Color = Color::from_rgb8(0xe8, 0xe1, 0xd0);
const L_BG_HOVER: Color = Color::from_rgb8(0xec, 0xe5, 0xd5); // hover fill
const L_AMBER_TEXT: Color = Color::from_rgb8(0xa3, 0x65, 0x0a); // accent TEXT (AA on paper)
const L_AMBER_FILL: Color = Color::from_rgb8(0xff, 0xb6, 0x27); // accent FILL (same as dark)
const L_CYAN: Color = Color::from_rgb8(0x0f, 0x7c, 0x8c); // info hue, darkened for AA
const L_ERROR: Color = Color::from_rgb8(0xc2, 0x48, 0x3f);
const L_FG: Color = Color::from_rgb8(0x22, 0x1d, 0x12); // primary text (warm ink)
const L_FG_MUTED: Color = Color::from_rgb8(0x65, 0x5d, 0x48);

const WHITE: Color = Color::from_rgb8(0xff, 0xff, 0xff);
const BLACK: Color = Color::from_rgb8(0x00, 0x00, 0x00);

impl ColorScheme {
    /// The Glyph **dark** color scheme — the canonical brightness. Maps
    /// `glyph-design-system.html`'s flat token set onto the 46 M3 roles per
    /// this module's accent-role convention and nearest-role mappings
    /// (retrieved 2026-07-21, RESEARCH §1.1).
    pub const fn glyph_dark() -> Self {
        Self {
            // Accent (text/icon) + bright-fill container split (see module docs).
            primary: D_AMBER,
            on_primary: D_ON_AMBER,
            primary_container: D_AMBER, // bright fill
            on_primary_container: D_ON_AMBER,
            // `*_fixed` are brightness-invariant by M3 definition — Frust-
            // authored from the amber family, identical in glyph_light.
            primary_fixed: FIXED_PRIMARY,
            primary_fixed_dim: FIXED_PRIMARY_DIM,
            on_primary_fixed: FIXED_ON_PRIMARY,
            on_primary_fixed_variant: FIXED_ON_PRIMARY_VARIANT,

            // Neutral "secondary" — Glyph has no second accent; borrow the
            // muted-fg / raised-surface family (secondary buttons are
            // bg-raised).
            secondary: D_FG_MUTED,
            on_secondary: D_BG_VOID,
            secondary_container: D_BG_RAISED,
            on_secondary_container: D_FG,
            secondary_fixed: FIXED_SECONDARY,
            secondary_fixed_dim: FIXED_SECONDARY_DIM,
            on_secondary_fixed: FIXED_ON_SECONDARY,
            on_secondary_fixed_variant: FIXED_ON_SECONDARY_VARIANT,

            // "tertiary" borrows the cyan info hue (Glyph's only spare hue).
            tertiary: D_CYAN,
            on_tertiary: D_BG_VOID,
            // info-faint rgba(94,200,216,.14) over bg-surface #161a23.
            tertiary_container: Color::from_rgb8(0x20, 0x32, 0x3C),
            on_tertiary_container: D_CYAN,
            tertiary_fixed: FIXED_TERTIARY,
            tertiary_fixed_dim: FIXED_TERTIARY_DIM,
            on_tertiary_fixed: FIXED_ON_TERTIARY,
            on_tertiary_fixed_variant: FIXED_ON_TERTIARY_VARIANT,

            error: D_ERROR,
            on_error: D_BG_VOID,
            // error-faint rgba(255,107,107,.12) over bg-surface #161a23.
            error_container: Color::from_rgb8(0x32, 0x24, 0x2C),
            on_error_container: D_ERROR,

            // Surface ramp: void→hover, monotonic (RESEARCH §1.1).
            surface: D_BG_SURFACE,
            on_surface: D_FG,
            on_surface_variant: D_FG_MUTED,
            surface_dim: D_BG_VOID,
            surface_bright: D_BG_HOVER,
            surface_container_lowest: D_BG_VOID,
            surface_container_low: D_BG_BASE,
            surface_container: D_BG_SURFACE,
            surface_container_high: D_BG_RAISED,
            surface_container_highest: D_BG_OVERLAY,

            // border-bright rgba(242,234,217,.18) / border .09, over #161a23.
            outline: Color::from_rgb8(0x3E, 0x3F, 0x44),
            outline_variant: Color::from_rgb8(0x2A, 0x2D, 0x33),
            shadow: BLACK,
            scrim: BLACK,
            // inverse_* reuse the opposite brightness's base tones.
            inverse_surface: D_FG,
            inverse_on_surface: D_BG_VOID,
            inverse_primary: L_AMBER_TEXT,
            surface_tint: D_AMBER,
        }
    }

    /// The Glyph **light** color scheme — "same tokens · accent re-tuned for
    /// AA on paper surfaces" (`glyph-design-system-light.html`, retrieved
    /// 2026-07-21, RESEARCH §1.1b). Not a value flip: the accent splits into a
    /// darkened text tone and the still-bright fill, the semantic hues darken,
    /// and the surface ramp is re-ordered (`surface` = white is the *lightest*
    /// slot, unlike dark's monotonic ramp).
    pub const fn glyph_light() -> Self {
        Self {
            // Accent split: text tone (#a3650a) vs still-bright fill (#ffb627).
            primary: L_AMBER_TEXT,
            on_primary: WHITE, // #a3650a is dark enough for white ink (AA ~4.75)
            primary_container: L_AMBER_FILL, // bright fill stays amber
            on_primary_container: L_ON_AMBER,
            primary_fixed: FIXED_PRIMARY,
            primary_fixed_dim: FIXED_PRIMARY_DIM,
            on_primary_fixed: FIXED_ON_PRIMARY,
            on_primary_fixed_variant: FIXED_ON_PRIMARY_VARIANT,

            secondary: L_FG_MUTED,
            on_secondary: WHITE,
            secondary_container: L_BG_RAISED,
            on_secondary_container: L_FG,
            secondary_fixed: FIXED_SECONDARY,
            secondary_fixed_dim: FIXED_SECONDARY_DIM,
            on_secondary_fixed: FIXED_ON_SECONDARY,
            on_secondary_fixed_variant: FIXED_ON_SECONDARY_VARIANT,

            tertiary: L_CYAN,
            on_tertiary: WHITE,
            // info-faint rgba(15,124,140,.10) over bg-surface #ffffff.
            tertiary_container: Color::from_rgb8(0xE7, 0xF2, 0xF4),
            on_tertiary_container: L_CYAN,
            tertiary_fixed: FIXED_TERTIARY,
            tertiary_fixed_dim: FIXED_TERTIARY_DIM,
            on_tertiary_fixed: FIXED_ON_TERTIARY,
            on_tertiary_fixed_variant: FIXED_ON_TERTIARY_VARIANT,

            error: L_ERROR,
            on_error: WHITE,
            // error-faint rgba(194,72,63,.10) over bg-surface #ffffff.
            error_container: Color::from_rgb8(0xF9, 0xED, 0xEC),
            on_error_container: L_ERROR,

            // Surface ramp: white is the *lightest* slot (RESEARCH §1.1b) —
            // the container ladder darkens as it rises (M3 light convention).
            surface: L_BG_SURFACE,
            on_surface: L_FG,
            on_surface_variant: L_FG_MUTED,
            surface_dim: L_BG_HOVER, // dimmest everyday surface
            surface_bright: L_BG_SURFACE,
            surface_container_lowest: L_BG_SURFACE, // #ffffff, lightest
            surface_container_low: L_BG_BASE,
            surface_container: L_BG_VOID, // the page tone
            surface_container_high: L_BG_RAISED,
            surface_container_highest: L_BG_OVERLAY, // darkest

            // border-bright rgba(34,29,18,.18) / border .10, over #ffffff.
            outline: Color::from_rgb8(0xD7, 0xD6, 0xD4),
            outline_variant: Color::from_rgb8(0xE9, 0xE8, 0xE7),
            // Light elevation reads as warm-ink lift, not black haze — the
            // shadow *role* is the warm ink (elevation alpha does the rest).
            shadow: L_FG,
            scrim: BLACK,
            inverse_surface: L_FG,
            inverse_on_surface: L_BG_VOID,
            inverse_primary: D_AMBER,
            surface_tint: L_AMBER_TEXT,
        }
    }
}

// ---- Brightness-invariant `*_fixed` derivations ------------------------
//
// M3's `*_fixed`/`*_fixed_dim`/`on_*_fixed(_variant)` roles are
// brightness-invariant by definition (identical in both schemes). Glyph has
// no source token for them, so these are Frust-authored from the amber
// (primary), muted-neutral (secondary), and cyan (tertiary) families —
// mirroring the documented approach [`ColorScheme::cupertino_light`] uses for
// iOS's own missing M3 concepts.

const FIXED_PRIMARY: Color = D_AMBER; // #ffb627 bright fill
const FIXED_PRIMARY_DIM: Color = D_AMBER_DIM; // #c98a12
const FIXED_ON_PRIMARY: Color = D_ON_AMBER; // #241a04
const FIXED_ON_PRIMARY_VARIANT: Color = Color::from_rgb8(0x3d, 0x2c, 0x08); // amber ink, one step up

const FIXED_SECONDARY: Color = Color::from_rgb8(0xf1, 0xec, 0xe0); // light neutral (both)
const FIXED_SECONDARY_DIM: Color = Color::from_rgb8(0xe8, 0xe1, 0xd0);
const FIXED_ON_SECONDARY: Color = Color::from_rgb8(0x22, 0x1d, 0x12); // warm ink
const FIXED_ON_SECONDARY_VARIANT: Color = Color::from_rgb8(0x65, 0x5d, 0x48);

const FIXED_TERTIARY: Color = D_CYAN; // #5ec8d8 bright cyan (both)
const FIXED_TERTIARY_DIM: Color = L_CYAN; // #0f7c8c
const FIXED_ON_TERTIARY: Color = Color::from_rgb8(0x00, 0x2b, 0x31); // deep cyan ink
const FIXED_ON_TERTIARY_VARIANT: Color = L_CYAN;

// ---- GlyphInk: brightness-invariant terminal/tooltip tokens ------------

/// The Glyph "ink" tokens: the terminal/code block and tooltip stay **dark
/// in both brightnesses** by source mandate (RESEARCH §1.1b(c)) — "shell
/// output is read against a dark background by habit and convention", and a
/// tooltip "is a floating overlay, not page content". These are
/// brightness-invariant, so they live in a [`crate::extensions::ThemeExtensions`]
/// entry ([`crate::theme::Theme::glyph_baseline`] attaches one) rather than
/// as brightness-swapped [`ColorScheme`] roles.
///
/// Values are the light source's dark-ink swatches
/// (`glyph-design-system-light.html`'s `.term-*`/`.tooltip-bubble` rules,
/// retrieved 2026-07-21) — the light build spells these out as literal dark
/// hexes precisely because they do not follow the page's brightness.
///
/// `Copy + Clone + Send + Sync` so it satisfies the extension-slot bounds and
/// a [`crate::theme::Theme`] clone stays cheap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphInk {
    /// Terminal/code block background (`.term-block` `#1c1912`).
    pub terminal_bg: Color,
    /// Terminal header bar (`.term-head` `#25211a`).
    pub terminal_head: Color,
    /// Terminal body text (`.term-body` `#d8cfb8`).
    pub terminal_fg: Color,
    /// Terminal command output, dimmer (`.term-out` `#a89e85`).
    pub terminal_out: Color,
    /// Terminal comment/annotation, dimmest (`.term-comment` `#6b6252`).
    pub terminal_comment: Color,
    /// Terminal prompt marker and blinking caret — the amber accent
    /// (`.term-prompt`/`.term-caret` `#ffb627`).
    pub terminal_prompt: Color,
    /// Tooltip bubble background (`.tooltip-bubble` `#2a2519`).
    pub tooltip_bg: Color,
    /// Tooltip text (`.tooltip-bubble` color `#f2ead9` — the dark-mode fg).
    pub tooltip_fg: Color,
}

impl GlyphInk {
    /// The default Glyph ink token set (RESEARCH §1.1b(c); retrieved
    /// 2026-07-21).
    pub const fn default_ink() -> Self {
        Self {
            terminal_bg: Color::from_rgb8(0x1c, 0x19, 0x12),
            terminal_head: Color::from_rgb8(0x25, 0x21, 0x1a),
            terminal_fg: Color::from_rgb8(0xd8, 0xcf, 0xb8),
            terminal_out: Color::from_rgb8(0xa8, 0x9e, 0x85),
            terminal_comment: Color::from_rgb8(0x6b, 0x62, 0x52),
            terminal_prompt: D_AMBER, // #ffb627
            tooltip_bg: Color::from_rgb8(0x2a, 0x25, 0x19),
            tooltip_fg: D_FG, // #f2ead9
        }
    }
}

impl Default for GlyphInk {
    fn default() -> Self {
        Self::default_ink()
    }
}

// ---- Glyph StatusPalette (success/warning/info) ------------------------

impl StatusPalette {
    /// The Glyph success/warning/info palette (RESEARCH §1.1/§1.1b; retrieved
    /// 2026-07-21). `info` maps to Glyph's `cyan` token. Container tints are
    /// each semantic hue's own faint wash pre-flattened over `bg-surface`
    /// (`#161a23` dark, `#ffffff` light) per this module's alpha convention;
    /// on-container ink is the semantic hue itself, matching how the source
    /// paints semantic text on its faint wash.
    pub const fn glyph() -> Self {
        Self {
            light: StatusColors {
                // success #2f8f5b, faint .10; warning #96690a, faint .12;
                // info(cyan) #0f7c8c, faint .10.
                success: Color::from_rgb8(0x2f, 0x8f, 0x5b),
                on_success: WHITE,
                success_container: Color::from_rgb8(0xEA, 0xF4, 0xEF),
                on_success_container: Color::from_rgb8(0x2f, 0x8f, 0x5b),

                warning: Color::from_rgb8(0x96, 0x69, 0x0a),
                on_warning: WHITE,
                warning_container: Color::from_rgb8(0xF2, 0xED, 0xE2),
                on_warning_container: Color::from_rgb8(0x96, 0x69, 0x0a),

                info: L_CYAN,
                on_info: WHITE,
                info_container: Color::from_rgb8(0xE7, 0xF2, 0xF4),
                on_info_container: L_CYAN,
            },
            dark: StatusColors {
                // success #5fd88f, faint .12; warning #f5c860, faint .12;
                // info(cyan) #5ec8d8, faint .14.
                success: Color::from_rgb8(0x5f, 0xd8, 0x8f),
                on_success: D_BG_VOID,
                success_container: Color::from_rgb8(0x1F, 0x31, 0x30),
                on_success_container: Color::from_rgb8(0x5f, 0xd8, 0x8f),

                warning: Color::from_rgb8(0xf5, 0xc8, 0x60),
                on_warning: D_BG_VOID,
                warning_container: Color::from_rgb8(0x31, 0x2F, 0x2A),
                on_warning_container: Color::from_rgb8(0xf5, 0xc8, 0x60),

                info: D_CYAN,
                on_info: D_BG_VOID,
                info_container: Color::from_rgb8(0x20, 0x32, 0x3C),
                on_info_container: D_CYAN,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Brightness;

    // ---- WCAG contrast helper (used by the AA tests below) -------------

    fn linearize(c: u8) -> f64 {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(color: Color) -> f64 {
        let [r, g, b, _] = color.to_rgba8().to_u8_array();
        0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
    }

    /// WCAG 2.x contrast ratio between two opaque colors (1.0..=21.0).
    fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Alpha-flatten `fg` over `bg` (straight-alpha), the exact operation the
    /// pre-flattened opaque roles above encode — lets a test ground a stored
    /// literal against the source `rgba()` alpha rather than a hand-copied hex.
    fn flatten(fg: Color, bg: Color, alpha: f64) -> Color {
        let [fr, fg_, fb, _] = fg.to_rgba8().to_u8_array();
        let [br, bg_, bb, _] = bg.to_rgba8().to_u8_array();
        let ch = |f: u8, b: u8| ((f as f64) * alpha + (b as f64) * (1.0 - alpha)).round() as u8;
        Color::from_rgb8(ch(fr, br), ch(fg_, bg_), ch(fb, bb))
    }

    fn assert_close(a: Color, b: Color) {
        let [ar, ag, ab, _] = a.to_rgba8().to_u8_array();
        let [br, bg, bb, _] = b.to_rgba8().to_u8_array();
        let d = |x: u8, y: u8| (x as i32 - y as i32).abs();
        assert!(
            d(ar, br) <= 1 && d(ag, bg) <= 1 && d(ab, bb) <= 1,
            "colors differ by >1/channel: {:?} vs {:?}",
            a.to_rgba8().to_u8_array(),
            b.to_rgba8().to_u8_array()
        );
    }

    const AA: f64 = 4.5;

    // ---- Dark scheme: field-by-field against `glyph-design-system.html` --

    #[test]
    fn glyph_dark_accent_split_matches_source() {
        let d = ColorScheme::glyph_dark();
        // primary = accent text/icon (amber); container = bright fill (amber);
        // on-container = near-black amber ink `.btn-primary`.
        assert_eq!(d.primary, Color::from_rgb8(0xff, 0xb6, 0x27));
        assert_eq!(d.primary_container, Color::from_rgb8(0xff, 0xb6, 0x27));
        assert_eq!(d.on_primary_container, Color::from_rgb8(0x24, 0x1a, 0x04));
        assert_eq!(d.surface_tint, d.primary);
    }

    #[test]
    fn glyph_dark_surface_ramp_matches_source() {
        let d = ColorScheme::glyph_dark();
        assert_eq!(
            d.surface_container_lowest,
            Color::from_rgb8(0x0a, 0x0c, 0x11)
        ); // void
        assert_eq!(d.surface_container_low, Color::from_rgb8(0x10, 0x13, 0x1a)); // base
        assert_eq!(d.surface, Color::from_rgb8(0x16, 0x1a, 0x23)); // surface
        assert_eq!(d.surface_container_high, Color::from_rgb8(0x1e, 0x23, 0x30)); // raised
        assert_eq!(
            d.surface_container_highest,
            Color::from_rgb8(0x27, 0x2d, 0x3d)
        ); // overlay
        assert_eq!(d.surface_bright, Color::from_rgb8(0x2f, 0x36, 0x48)); // hover
    }

    #[test]
    fn glyph_dark_foreground_and_semantic_match_source() {
        let d = ColorScheme::glyph_dark();
        assert_eq!(d.on_surface, Color::from_rgb8(0xf2, 0xea, 0xd9)); // fg
        assert_eq!(d.on_surface_variant, Color::from_rgb8(0xa3, 0x9c, 0x88)); // fg-muted
        assert_eq!(d.error, Color::from_rgb8(0xff, 0x6b, 0x6b));
        assert_eq!(d.tertiary, Color::from_rgb8(0x5e, 0xc8, 0xd8)); // cyan/info
    }

    #[test]
    fn glyph_dark_alpha_roles_flatten_from_source_over_surface() {
        let d = ColorScheme::glyph_dark();
        let surface = Color::from_rgb8(0x16, 0x1a, 0x23);
        // outline = border-bright rgba(242,234,217,.18); variant = border .09.
        let bfg = Color::from_rgb8(0xf2, 0xea, 0xd9);
        assert_close(d.outline, flatten(bfg, surface, 0.18));
        assert_close(d.outline_variant, flatten(bfg, surface, 0.09));
        // error_container = error-faint rgba(255,107,107,.12).
        assert_close(
            d.error_container,
            flatten(Color::from_rgb8(0xff, 0x6b, 0x6b), surface, 0.12),
        );
        // tertiary_container = info(cyan)-faint rgba(94,200,216,.14).
        assert_close(
            d.tertiary_container,
            flatten(Color::from_rgb8(0x5e, 0xc8, 0xd8), surface, 0.14),
        );
    }

    // ---- Light scheme: field-by-field against the -light.html source -----

    #[test]
    fn glyph_light_accent_split_matches_source() {
        let l = ColorScheme::glyph_light();
        assert_eq!(l.primary, Color::from_rgb8(0xa3, 0x65, 0x0a)); // text tone
        assert_eq!(l.primary_container, Color::from_rgb8(0xff, 0xb6, 0x27)); // bright fill
        assert_eq!(l.on_primary_container, Color::from_rgb8(0x2a, 0x1c, 0x04));
        assert_eq!(l.surface_tint, l.primary);
    }

    #[test]
    fn glyph_light_surface_is_lightest_and_ramp_darkens_upward() {
        let l = ColorScheme::glyph_light();
        // Deliberate: `surface` = white is the lightest slot (RESEARCH §1.1b),
        // unlike dark's monotonic ramp.
        assert_eq!(l.surface, Color::from_rgb8(0xff, 0xff, 0xff));
        assert_eq!(
            l.surface_container_lowest,
            Color::from_rgb8(0xff, 0xff, 0xff)
        );
        assert_eq!(l.surface_container_low, Color::from_rgb8(0xfd, 0xfb, 0xf6));
        assert_eq!(l.surface_container, Color::from_rgb8(0xf6, 0xf2, 0xe9)); // page
        assert_eq!(l.surface_container_high, Color::from_rgb8(0xf1, 0xec, 0xe0));
        assert_eq!(
            l.surface_container_highest,
            Color::from_rgb8(0xe8, 0xe1, 0xd0)
        );
    }

    #[test]
    fn glyph_light_foreground_and_semantic_match_source() {
        let l = ColorScheme::glyph_light();
        assert_eq!(l.on_surface, Color::from_rgb8(0x22, 0x1d, 0x12)); // fg
        assert_eq!(l.on_surface_variant, Color::from_rgb8(0x65, 0x5d, 0x48)); // fg-muted
        assert_eq!(l.error, Color::from_rgb8(0xc2, 0x48, 0x3f));
        assert_eq!(l.tertiary, Color::from_rgb8(0x0f, 0x7c, 0x8c)); // cyan/info
    }

    #[test]
    fn glyph_light_alpha_roles_flatten_from_source_over_white() {
        let l = ColorScheme::glyph_light();
        let surface = Color::from_rgb8(0xff, 0xff, 0xff);
        let bfg = Color::from_rgb8(0x22, 0x1d, 0x12); // warm ink border color
        assert_close(l.outline, flatten(bfg, surface, 0.18));
        assert_close(l.outline_variant, flatten(bfg, surface, 0.10));
        assert_close(
            l.error_container,
            flatten(Color::from_rgb8(0xc2, 0x48, 0x3f), surface, 0.10),
        );
    }

    // ---- Fixed-role brightness invariance -------------------------------

    #[test]
    fn fixed_roles_are_brightness_invariant() {
        let d = ColorScheme::glyph_dark();
        let l = ColorScheme::glyph_light();
        assert_eq!(d.primary_fixed, l.primary_fixed);
        assert_eq!(d.primary_fixed_dim, l.primary_fixed_dim);
        assert_eq!(d.on_primary_fixed, l.on_primary_fixed);
        assert_eq!(d.on_primary_fixed_variant, l.on_primary_fixed_variant);
        assert_eq!(d.secondary_fixed, l.secondary_fixed);
        assert_eq!(d.secondary_fixed_dim, l.secondary_fixed_dim);
        assert_eq!(d.tertiary_fixed, l.tertiary_fixed);
        assert_eq!(d.tertiary_fixed_dim, l.tertiary_fixed_dim);
    }

    // ---- WCAG AA contrast (acceptance criterion 2) ----------------------

    #[test]
    fn aa_primary_on_surface_both_brightnesses() {
        let d = ColorScheme::glyph_dark();
        let l = ColorScheme::glyph_light();
        assert!(
            contrast(d.primary, d.surface) >= AA,
            "dark primary-on-surface"
        );
        assert!(
            contrast(l.primary, l.surface) >= AA,
            "light primary-on-surface"
        );
    }

    #[test]
    fn aa_on_primary_container_on_container_both_brightnesses() {
        let d = ColorScheme::glyph_dark();
        let l = ColorScheme::glyph_light();
        assert!(
            contrast(d.on_primary_container, d.primary_container) >= AA,
            "dark on-container ink on the amber fill"
        );
        assert!(
            contrast(l.on_primary_container, l.primary_container) >= AA,
            "light on-container ink on the amber fill"
        );
    }

    #[test]
    fn aa_fg_ramp_on_surfaces_both_brightnesses() {
        // The primary + muted foreground ramp must clear AA over both the
        // canonical surface and the page-level container.
        let d = ColorScheme::glyph_dark();
        let l = ColorScheme::glyph_light();
        for (fg, bg) in [
            (d.on_surface, d.surface),
            (d.on_surface_variant, d.surface),
            (d.on_surface, d.surface_container_lowest),
        ] {
            assert!(contrast(fg, bg) >= AA, "dark fg ramp");
        }
        for (fg, bg) in [
            (l.on_surface, l.surface),
            (l.on_surface_variant, l.surface),
            (l.on_surface, l.surface_container),
        ] {
            assert!(contrast(fg, bg) >= AA, "light fg ramp");
        }
    }

    // ---- GlyphInk -------------------------------------------------------

    #[test]
    fn glyph_ink_matches_source_dark_swatches() {
        let ink = GlyphInk::default_ink();
        assert_eq!(ink.terminal_bg, Color::from_rgb8(0x1c, 0x19, 0x12));
        assert_eq!(ink.terminal_head, Color::from_rgb8(0x25, 0x21, 0x1a));
        assert_eq!(ink.terminal_fg, Color::from_rgb8(0xd8, 0xcf, 0xb8));
        assert_eq!(ink.terminal_out, Color::from_rgb8(0xa8, 0x9e, 0x85));
        assert_eq!(ink.terminal_comment, Color::from_rgb8(0x6b, 0x62, 0x52));
        assert_eq!(ink.terminal_prompt, Color::from_rgb8(0xff, 0xb6, 0x27));
        assert_eq!(ink.tooltip_bg, Color::from_rgb8(0x2a, 0x25, 0x19));
        assert_eq!(ink.tooltip_fg, Color::from_rgb8(0xf2, 0xea, 0xd9));
    }

    #[test]
    fn glyph_ink_terminal_text_is_aa_on_terminal_bg() {
        // The dark ink stays legible regardless of page brightness (its whole
        // reason for being brightness-invariant).
        let ink = GlyphInk::default_ink();
        assert!(contrast(ink.terminal_fg, ink.terminal_bg) >= AA);
    }

    #[test]
    fn glyph_ink_is_const_and_copy() {
        const INK: GlyphInk = GlyphInk::default_ink();
        let copied = INK;
        assert_eq!(copied, INK);
        assert_eq!(GlyphInk::default(), INK);
    }

    // ---- StatusPalette::glyph -------------------------------------------

    #[test]
    fn glyph_status_selects_by_brightness_and_matches_source() {
        let p = StatusPalette::glyph();
        assert_eq!(p.colors(Brightness::Dark), &p.dark);
        assert_eq!(p.colors(Brightness::Light), &p.light);
        // Dark hues.
        assert_eq!(p.dark.success, Color::from_rgb8(0x5f, 0xd8, 0x8f));
        assert_eq!(p.dark.warning, Color::from_rgb8(0xf5, 0xc8, 0x60));
        assert_eq!(p.dark.info, Color::from_rgb8(0x5e, 0xc8, 0xd8)); // cyan
        // Light hues (darkened for AA).
        assert_eq!(p.light.success, Color::from_rgb8(0x2f, 0x8f, 0x5b));
        assert_eq!(p.light.warning, Color::from_rgb8(0x96, 0x69, 0x0a));
        assert_eq!(p.light.info, Color::from_rgb8(0x0f, 0x7c, 0x8c));
    }

    #[test]
    fn glyph_status_containers_flatten_from_source_alpha() {
        let p = StatusPalette::glyph();
        let d_surface = Color::from_rgb8(0x16, 0x1a, 0x23);
        assert_close(
            p.dark.success_container,
            flatten(Color::from_rgb8(0x5f, 0xd8, 0x8f), d_surface, 0.12),
        );
        assert_close(
            p.dark.warning_container,
            flatten(Color::from_rgb8(0xf5, 0xc8, 0x60), d_surface, 0.12),
        );
        let l_surface = Color::from_rgb8(0xff, 0xff, 0xff);
        assert_close(
            p.light.success_container,
            flatten(Color::from_rgb8(0x2f, 0x8f, 0x5b), l_surface, 0.10),
        );
    }

    #[test]
    fn glyph_status_is_const_constructible() {
        const P: StatusPalette = StatusPalette::glyph();
        assert_eq!(P.dark.success, Color::from_rgb8(0x5f, 0xd8, 0x8f));
    }
}
