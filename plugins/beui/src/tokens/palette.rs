//! The vendored beUI token tables: one [`BeuiPalette`] per brightness,
//! transcribed from beUI's own stylesheet.
//!
//! **Source:** `app/globals.css` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved **2026-09-01** — its
//! `:root` block (light) and its `.dark` block (dark). Those two blocks are the
//! whole contract: the `@theme inline` block below them only re-publishes the
//! same custom properties under Tailwind's `--color-*` names, and adds no value
//! of its own.
//!
//! # What a brightness carries
//!
//! beUI authors a **small** base and derives the rest. Six values are authored
//! per brightness (`background`, `foreground`, `card`, `muted-foreground`,
//! `border`, `border-strong`), five brand hues are authored **once** in `:root`
//! and inherited by both brightnesses (`neon`, `violet`, `danger`, `success`,
//! `warning`), the glass/gradient groups are authored per brightness, and
//! everything else is a `var()` alias onto those (`--primary: var(--foreground)`,
//! `--input: var(--border)`, …).
//!
//! This module ports the **resolved** values: a CSS custom property is
//! substituted where it is *used*, so `--primary: var(--foreground)` declared in
//! `:root` still resolves to the dark `--foreground` inside `.dark`. Each alias
//! field below therefore carries the brightness-correct value and names the
//! alias it came from. The five brand hues genuinely do not vary — `.dark`
//! never redeclares them — so both tables carry the same literals, and that
//! sameness is the source's, not a copy-paste here.
//!
//! # OKLCH → sRGB conversion (authoring-time)
//!
//! Most upstream values are CSS `oklch()`; frust's [`Color`] is sRGB. Converting
//! at *runtime* would mean either a color-science dependency or hand-rolled
//! matrix math on every theme build, so the conversion ran **once, at authoring
//! time**, and this module embeds the resulting 8-bit sRGB literals with the
//! original CSS value as a trailing comment on every line — the source of truth
//! stays visible next to the value it produced. (Where upstream authors a plain
//! `#rrggbb` or `rgb()` value — dark `--background`, `--card`, and every dark
//! glass wash — there is nothing to convert and the comment names the source
//! form verbatim.)
//!
//! The conversion is Björn Ottosson's reference Oklab pipeline (the same one
//! the CSS Color 4 specification's sample code implements, and the same one
//! `frust-shadcn`'s palette records):
//!
//! 1. `oklch(L C H)` → Oklab: `a = C·cos(H)`, `b = C·sin(H)` (H in radians).
//! 2. Oklab → LMS′ via the `M2⁻¹` matrix, then cube each component.
//! 3. LMS → linear sRGB via the `M1⁻¹` matrix.
//! 4. Clamp each linear channel into `[0, 1]`, apply the sRGB transfer function
//!    (`12.92·c` below `0.0031308`, else `1.055·c^(1/2.4) − 0.055`), and round
//!    to 8 bits.
//! 5. An `oklch(… / A)` alpha form keeps its alpha as straight
//!    (un-premultiplied) alpha — [`Color::new`]'s fourth component.
//!
//! The pipeline was validated against published hexes before any beUI value ran
//! through it (`oklch(0.577 0.245 27.325)` → `#E7000B`, `oklch(0.145 0 0)` →
//! `#0A0A0A`, `oklch(0.553 0.013 58.071)` → `#79716B`), so a wrong value here is
//! a transcription error, not a math error.
//!
//! ## The gamut clamp is load-bearing here
//!
//! beUI's accent hues are authored at chroma `0.18`–`0.22`, which is **outside
//! the sRGB gamut** at their lightness for five of them — the cyan accent, the
//! violet, and the success/warning brand hues. Step 4's clamp is what makes them
//! renderable, and it is a real (documented) degrade, not a rounding artifact:
//! the clamped sRGB result is duller than the authored P3-ish color a browser on
//! a wide-gamut display shows. Every clamped line is marked
//! `(out of sRGB gamut, clamped)` in its trailing comment. This is a larger
//! deviation surface than `frust-shadcn` carries — that catalog clamps exactly
//! one role per preset, an achromatic palette otherwise — and it is the single
//! most likely place for a reviewer to find visible drift from the web original.
//!
//! # Alpha tokens are kept as alpha
//!
//! `border`, `border_strong`, the glass washes, and the glass hairline are all
//! authored as translucent washes over whatever is behind them
//! (`oklch(15% 0 0 / 0.06)` light, `rgb(255 255 255 / 0.05)` dark). They stay
//! translucent here rather than being pre-flattened over a surface — a beUI
//! border composites, and this catalog's call is "stay faithful to the source"
//! (`docs/WIDGETS_CODE_STANDARDS.md`'s alpha-border rule makes this a per-widget
//! call; the whole catalog answers it the same way).

use frust::Color;

/// The `--glass-*` group of one brightness: the three translucent panel washes
/// beUI's `.glass` / `.glass-strong` / `.glass-thin` utilities fill with, plus
/// the hairline all three stroke with.
///
/// Field names drop the `glass-` prefix the CSS keys carry (`glass-bg` becomes
/// `bg`), since the struct name already says "glass".
///
/// The blur radius each utility pairs its wash with is *not* here — it is a
/// material property rather than a color, and it lives on the assembled theme's
/// `GlassScale` (see [`glass_scale`](super::theme::glass_scale)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeuiGlass {
    /// `--glass-bg`: the `.glass` utility's wash — the panel tier (a popover,
    /// sheet or menu), and the only tier upstream gives a shadow.
    pub bg: Color,
    /// `--glass-strong-bg`: the `.glass-strong` utility's wash — the most
    /// opaque of the three.
    pub strong_bg: Color,
    /// `--glass-thin-bg`: the `.glass-thin` utility's wash — the sheerest.
    pub thin_bg: Color,
    /// `--glass-border`: the hairline `.glass` and `.glass-thin` stroke
    /// themselves with (`1px solid`).
    pub border: Color,
}

/// A two-stop linear gradient, as beUI authors it.
///
/// Both of beUI's gradients are exactly two stops at `0%`/`100%`, so this
/// models that shape rather than a general stop list — an n-stop gradient is
/// not something the source asks for, and inventing one would be inventing
/// vocabulary.
///
/// `angle_degrees` is the CSS `linear-gradient()` angle: **0° points up and
/// angles increase clockwise**, so `135deg` runs top-left → bottom-right and
/// `to right` is `90deg` (which is how [`BeuiPalette::gradient_accent`] records
/// its own direction — the source writes the keyword, this records the
/// equivalent angle so one field type covers both).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeuiGradient {
    /// The `0%` stop.
    pub from: Color,
    /// The `100%` stop.
    pub to: Color,
    /// CSS gradient angle in degrees (0° = up, clockwise).
    pub angle_degrees: f64,
}

/// beUI's token table for a **single brightness** — the resolved CSS custom
/// properties, one Rust field per key.
///
/// The pair is [`BEUI_LIGHT`]/[`BEUI_DARK`];
/// [`color_scheme`](super::theme::color_scheme) is what folds a pair onto
/// frust's 46-role `ColorScheme`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeuiPalette {
    // ---- Authored base ---------------------------------------------------
    /// `--background`: the app's base surface.
    pub background: Color,
    /// `--foreground`: default ink on `background`.
    pub foreground: Color,
    /// `--card`: a raised content panel's surface.
    pub card: Color,
    /// `--muted-foreground`: the dimmed ink role.
    pub muted_foreground: Color,
    /// `--border`: hairline rules and control borders — a translucent wash, not
    /// an opaque grey.
    pub border: Color,
    /// `--border-strong`: the emphasized hairline (roughly double `border`'s
    /// alpha). beUI has no separate focus-ring color; `--ring` aliases this.
    pub border_strong: Color,

    // ---- Authored accent + brand hues ------------------------------------
    /// `--accent`: beUI's signature cyan. The one hue that differs between
    /// brightnesses (the dark table lifts its lightness).
    pub accent: Color,
    /// `--accent-fg`: ink on `accent`.
    pub accent_fg: Color,
    /// `--neon`: a brand green. Decorative — no semantic role.
    pub neon: Color,
    /// `--violet`: a brand violet. Decorative — no semantic role.
    pub violet: Color,
    /// `--danger`: the destructive/error hue.
    pub danger: Color,
    /// `--success`: the success status hue.
    pub success: Color,
    /// `--warning`: the warning status hue.
    pub warning: Color,

    // ---- Authored glass + gradients --------------------------------------
    /// The `--glass-*` group.
    pub glass: BeuiGlass,
    /// `--gradient-bg`: the ambient page-background gradient.
    pub gradient_bg: BeuiGradient,
    /// `--gradient-accent`: the brand accent gradient (cyan → violet).
    pub gradient_accent: BeuiGradient,

    // ---- Resolved `var()` aliases ----------------------------------------
    /// `--card-foreground: var(--foreground)`.
    pub card_foreground: Color,
    /// `--popover: var(--card)`.
    pub popover: Color,
    /// `--popover-foreground: var(--foreground)`.
    pub popover_foreground: Color,
    /// `--primary: var(--foreground)` — beUI's solid fill *is* the ink color, so
    /// a primary button is near-black on light and near-white on dark.
    pub primary: Color,
    /// `--primary-foreground: var(--background)`.
    pub primary_foreground: Color,
    /// `--secondary: var(--card)`.
    pub secondary: Color,
    /// `--secondary-foreground: var(--foreground)`.
    pub secondary_foreground: Color,
    /// `--muted: var(--card)`.
    pub muted: Color,
    /// `--accent-foreground: var(--accent-fg)`.
    pub accent_foreground: Color,
    /// `--destructive: var(--danger)`.
    pub destructive: Color,
    /// `--input: var(--border)`.
    pub input: Color,
    /// `--ring: var(--border-strong)` — the focus-ring color, painted at
    /// reduced opacity by the controls that use it (see [`crate::style`]).
    pub ring: Color,
}

/// Return the straight-alpha color `#rrggbb` at `alpha` — the shape every
/// translucent upstream wash (`oklch(… / A)`, `rgb(… / A)`) transcribes to.
const fn rgba8(r: u8, g: u8, b: u8, alpha: f32) -> Color {
    let c = Color::from_rgb8(r, g, b).components;
    Color::new([c[0], c[1], c[2], alpha])
}

// The five brand hues `:root` authors and `.dark` never redeclares, so both
// tables below bind the same constants rather than repeating the literals and
// inviting the two copies to drift.
const NEON: Color = Color::from_rgb8(0x45, 0xE0, 0x59); // oklch(80% 0.22 145)
const VIOLET: Color = Color::from_rgb8(0xA6, 0x72, 0xFF); // oklch(68% 0.22 295) (out of sRGB gamut, clamped)
const DANGER: Color = Color::from_rgb8(0xEE, 0x34, 0x3B); // oklch(62% 0.22 25)
const SUCCESS: Color = Color::from_rgb8(0x00, 0xBE, 0x6A); // oklch(70% 0.18 155) (out of sRGB gamut, clamped)
const WARNING: Color = Color::from_rgb8(0xF9, 0xA3, 0x00); // oklch(78% 0.18 75) (out of sRGB gamut, clamped)

/// beUI's light-mode token table — the `:root` block, resolved.
pub const BEUI_LIGHT: BeuiPalette = BeuiPalette {
    background: Color::from_rgb8(0xFC, 0xFC, 0xFC), // oklch(99% 0 0)
    foreground: Color::from_rgb8(0x0B, 0x0B, 0x0B), // oklch(15% 0 0)
    card: Color::from_rgb8(0xF5, 0xF5, 0xF5),       // oklch(97% 0 0)
    muted_foreground: Color::from_rgb8(0x63, 0x63, 0x63), // oklch(50% 0 0)
    border: rgba8(0x0B, 0x0B, 0x0B, 0.06),          // oklch(15% 0 0 / 0.06)
    border_strong: rgba8(0x0B, 0x0B, 0x0B, 0.12),   // oklch(15% 0 0 / 0.12)

    accent: Color::from_rgb8(0x00, 0xC5, 0xC7), // oklch(72% 0.18 195) (out of sRGB gamut, clamped)
    accent_fg: Color::from_rgb8(0x0B, 0x0B, 0x0B), // --accent-fg: oklch(15% 0 0)
    neon: NEON,
    violet: VIOLET,
    danger: DANGER,
    success: SUCCESS,
    warning: WARNING,

    glass: BeuiGlass {
        bg: rgba8(0xFC, 0xFC, 0xFC, 0.55), // --glass-bg: oklch(99% 0 0 / 0.55)
        strong_bg: rgba8(0xFF, 0xFF, 0xFF, 0.7), // --glass-strong-bg: rgb(255 255 255 / 0.7)
        thin_bg: rgba8(0xFF, 0xFF, 0xFF, 0.45), // --glass-thin-bg: rgb(255 255 255 / 0.45)
        border: rgba8(0x0B, 0x0B, 0x0B, 0.08), // --glass-border: oklch(15% 0 0 / 0.08)
    },
    gradient_bg: BeuiGradient {
        from: Color::from_rgb8(0xFC, 0xFC, 0xFC), // oklch(99% 0 0) at 0%
        to: Color::from_rgb8(0xEC, 0xF3, 0xF5),   // oklch(96% 0.008 210) at 100%
        angle_degrees: 135.0,                     // linear-gradient(135deg, …)
    },
    gradient_accent: BeuiGradient {
        from: Color::from_rgb8(0x00, 0xC5, 0xC7), // oklch(72% 0.18 195) at 0% (out of sRGB gamut, clamped)
        to: Color::from_rgb8(0xA6, 0x72, 0xFF), // oklch(68% 0.22 295) at 100% (out of sRGB gamut, clamped)
        angle_degrees: 90.0,                    // linear-gradient(to right, …)
    },

    // Resolved `var()` aliases — each takes the light value of the base token
    // it names.
    card_foreground: Color::from_rgb8(0x0B, 0x0B, 0x0B), // var(--foreground)
    popover: Color::from_rgb8(0xF5, 0xF5, 0xF5),         // var(--card)
    popover_foreground: Color::from_rgb8(0x0B, 0x0B, 0x0B), // var(--foreground)
    primary: Color::from_rgb8(0x0B, 0x0B, 0x0B),         // var(--foreground)
    primary_foreground: Color::from_rgb8(0xFC, 0xFC, 0xFC), // var(--background)
    secondary: Color::from_rgb8(0xF5, 0xF5, 0xF5),       // var(--card)
    secondary_foreground: Color::from_rgb8(0x0B, 0x0B, 0x0B), // var(--foreground)
    muted: Color::from_rgb8(0xF5, 0xF5, 0xF5),           // var(--card)
    accent_foreground: Color::from_rgb8(0x0B, 0x0B, 0x0B), // var(--accent-fg)
    destructive: DANGER,                                 // var(--danger)
    input: rgba8(0x0B, 0x0B, 0x0B, 0.06),                // var(--border)
    ring: rgba8(0x0B, 0x0B, 0x0B, 0.12),                 // var(--border-strong)
};

/// beUI's dark-mode token table — the `.dark` block, resolved (every key it
/// does not redeclare inherits `:root`'s, and every `var()` alias re-resolves
/// against the dark base).
pub const BEUI_DARK: BeuiPalette = BeuiPalette {
    background: Color::from_rgb8(0x15, 0x15, 0x15), // #151515
    foreground: Color::from_rgb8(0xF2, 0xF2, 0xF2), // oklch(96% 0 0)
    card: Color::from_rgb8(0x1C, 0x1C, 0x1C),       // #1c1c1c
    muted_foreground: Color::from_rgb8(0x86, 0x86, 0x86), // oklch(62% 0 0)
    border: rgba8(0xFF, 0xFF, 0xFF, 0.05),          // rgb(255 255 255 / 0.05)
    border_strong: rgba8(0xFF, 0xFF, 0xFF, 0.1),    // rgb(255 255 255 / 0.1)

    accent: Color::from_rgb8(0x00, 0xDF, 0xE1), // oklch(80% 0.18 195) (out of sRGB gamut, clamped)
    accent_fg: Color::from_rgb8(0x15, 0x15, 0x15), // --accent-fg: #151515
    // Inherited from `:root` — `.dark` redeclares none of these.
    neon: NEON,
    violet: VIOLET,
    danger: DANGER,
    success: SUCCESS,
    warning: WARNING,

    glass: BeuiGlass {
        bg: rgba8(0x1C, 0x1C, 0x1C, 0.55), // --glass-bg: rgb(28 28 28 / 0.55)
        strong_bg: rgba8(0x1C, 0x1C, 0x1C, 0.6), // --glass-strong-bg: rgb(28 28 28 / 0.6)
        thin_bg: rgba8(0x15, 0x15, 0x15, 0.45), // --glass-thin-bg: rgb(21 21 21 / 0.45)
        border: rgba8(0xFF, 0xFF, 0xFF, 0.08), // --glass-border: rgb(255 255 255 / 0.08)
    },
    gradient_bg: BeuiGradient {
        from: Color::from_rgb8(0x14, 0x14, 0x14), // oklch(19% 0 0) at 0%
        to: Color::from_rgb8(0x16, 0x1C, 0x1D),   // oklch(22% 0.008 210) at 100%
        angle_degrees: 135.0,                     // linear-gradient(135deg, …)
    },
    gradient_accent: BeuiGradient {
        from: Color::from_rgb8(0x00, 0xDF, 0xE1), // oklch(80% 0.18 195) at 0% (out of sRGB gamut, clamped)
        to: Color::from_rgb8(0xAE, 0x89, 0xFF), // oklch(72% 0.18 295) at 100% (out of sRGB gamut, clamped)
        angle_degrees: 90.0,                    // linear-gradient(to right, …)
    },

    // Resolved `var()` aliases — each takes the dark value of the base token it
    // names, which is the whole reason the alias exists upstream.
    card_foreground: Color::from_rgb8(0xF2, 0xF2, 0xF2), // var(--foreground)
    popover: Color::from_rgb8(0x1C, 0x1C, 0x1C),         // var(--card)
    popover_foreground: Color::from_rgb8(0xF2, 0xF2, 0xF2), // var(--foreground)
    primary: Color::from_rgb8(0xF2, 0xF2, 0xF2),         // var(--foreground)
    primary_foreground: Color::from_rgb8(0x15, 0x15, 0x15), // var(--background)
    secondary: Color::from_rgb8(0x1C, 0x1C, 0x1C),       // var(--card)
    secondary_foreground: Color::from_rgb8(0xF2, 0xF2, 0xF2), // var(--foreground)
    muted: Color::from_rgb8(0x1C, 0x1C, 0x1C),           // var(--card)
    accent_foreground: Color::from_rgb8(0x15, 0x15, 0x15), // var(--accent-fg)
    destructive: DANGER,                                 // var(--danger)
    input: rgba8(0xFF, 0xFF, 0xFF, 0.05),                // var(--border)
    ring: rgba8(0xFF, 0xFF, 0xFF, 0.1),                  // var(--border-strong)
};

/// The token table for `brightness` — the light/dark selector every consumer
/// resolves through, so no caller has to name the two constants itself.
pub const fn palette(brightness: frust::Brightness) -> BeuiPalette {
    match brightness {
        frust::Brightness::Light => BEUI_LIGHT,
        frust::Brightness::Dark => BEUI_DARK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;

    #[test]
    fn the_brightness_selector_returns_the_matching_table() {
        assert_eq!(palette(Brightness::Light), BEUI_LIGHT);
        assert_eq!(palette(Brightness::Dark), BEUI_DARK);
        // The two really are different tables, so a component that forgets to
        // thread brightness cannot accidentally look correct.
        assert_ne!(BEUI_LIGHT, BEUI_DARK);
    }

    /// The `var()` aliases are the half of this transcription most likely to
    /// drift: upstream declares them once in `:root` and lets CSS substitution
    /// re-resolve them per brightness, while this port writes each resolved
    /// value out by hand. This pins every alias to the base token it names, in
    /// both brightnesses.
    #[test]
    fn every_var_alias_resolves_to_its_base_token_in_both_schemes() {
        for p in [BEUI_LIGHT, BEUI_DARK] {
            assert_eq!(p.card_foreground, p.foreground, "--card-foreground");
            assert_eq!(p.popover, p.card, "--popover");
            assert_eq!(p.popover_foreground, p.foreground, "--popover-foreground");
            assert_eq!(p.primary, p.foreground, "--primary");
            assert_eq!(p.primary_foreground, p.background, "--primary-foreground");
            assert_eq!(p.secondary, p.card, "--secondary");
            assert_eq!(
                p.secondary_foreground, p.foreground,
                "--secondary-foreground"
            );
            assert_eq!(p.muted, p.card, "--muted");
            assert_eq!(p.accent_foreground, p.accent_fg, "--accent-foreground");
            assert_eq!(p.destructive, p.danger, "--destructive");
            assert_eq!(p.input, p.border, "--input");
            assert_eq!(p.ring, p.border_strong, "--ring");
        }
    }

    /// `:root` authors the five brand hues and `.dark` never redeclares them,
    /// so they are brightness-invariant by construction. A future edit that
    /// gives one of them a dark variant has to change this test deliberately.
    #[test]
    fn the_brand_hues_are_inherited_by_dark_mode_unchanged() {
        assert_eq!(BEUI_LIGHT.neon, BEUI_DARK.neon);
        assert_eq!(BEUI_LIGHT.violet, BEUI_DARK.violet);
        assert_eq!(BEUI_LIGHT.danger, BEUI_DARK.danger);
        assert_eq!(BEUI_LIGHT.success, BEUI_DARK.success);
        assert_eq!(BEUI_LIGHT.warning, BEUI_DARK.warning);
        // The accent is the one hue that *does* vary — `.dark` lifts it.
        assert_ne!(BEUI_LIGHT.accent, BEUI_DARK.accent);
    }

    /// Borders and glass washes are authored as translucent washes upstream and
    /// stay translucent here (the module docs' alpha rule). Flattening one over
    /// a surface would make it wrong over every other backdrop.
    #[test]
    fn borders_and_glass_washes_stay_translucent() {
        for p in [BEUI_LIGHT, BEUI_DARK] {
            for (name, color) in [
                ("border", p.border),
                ("border_strong", p.border_strong),
                ("input", p.input),
                ("ring", p.ring),
                ("glass.bg", p.glass.bg),
                ("glass.strong_bg", p.glass.strong_bg),
                ("glass.thin_bg", p.glass.thin_bg),
                ("glass.border", p.glass.border),
            ] {
                let alpha = color.components[3];
                assert!(
                    alpha > 0.0 && alpha < 1.0,
                    "{name} must stay a translucent wash, got alpha {alpha}"
                );
            }
            // `border_strong` is the emphasized rule: strictly more opaque.
            assert!(p.border_strong.components[3] > p.border.components[3]);
        }
    }

    /// Every opaque role is fully opaque — the counterpart of the wash test, so
    /// a stray alpha in a surface or ink literal is caught rather than quietly
    /// letting the page show through a button.
    #[test]
    fn every_opaque_role_is_fully_opaque() {
        for p in [BEUI_LIGHT, BEUI_DARK] {
            for (name, color) in [
                ("background", p.background),
                ("foreground", p.foreground),
                ("card", p.card),
                ("muted_foreground", p.muted_foreground),
                ("accent", p.accent),
                ("accent_fg", p.accent_fg),
                ("neon", p.neon),
                ("violet", p.violet),
                ("danger", p.danger),
                ("success", p.success),
                ("warning", p.warning),
                ("primary", p.primary),
                ("primary_foreground", p.primary_foreground),
            ] {
                assert_eq!(color.components[3], 1.0, "{name} must be opaque");
            }
        }
    }

    /// Both schemes resolve to a readable base pair: light mode is dark ink on
    /// a light surface and dark mode is the reverse. A transposed table (the
    /// classic transcription slip) fails here.
    #[test]
    fn both_schemes_resolve_the_right_way_round() {
        let luma = |c: Color| {
            let [r, g, b, _] = c.components;
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        assert!(luma(BEUI_LIGHT.background) > luma(BEUI_LIGHT.foreground));
        assert!(luma(BEUI_DARK.background) < luma(BEUI_DARK.foreground));
        // The card surface sits between background and ink in both schemes —
        // upstream's `--card` is a *near*-background panel, not a contrast step.
        for p in [BEUI_LIGHT, BEUI_DARK] {
            let (bg, card, fg) = (luma(p.background), luma(p.card), luma(p.foreground));
            assert!((card - bg).abs() < (fg - bg).abs());
        }
    }

    /// The gradients carry the directions the source writes: `135deg` for the
    /// ambient background gradient and `to right` (= 90°) for the accent one.
    #[test]
    fn gradients_carry_their_source_directions() {
        for p in [BEUI_LIGHT, BEUI_DARK] {
            assert_eq!(p.gradient_bg.angle_degrees, 135.0);
            assert_eq!(p.gradient_accent.angle_degrees, 90.0);
            // The accent gradient starts at the scheme's own accent hue.
            assert_eq!(p.gradient_accent.from, p.accent);
        }
        // The violet end is the shared brand violet in light mode; dark mode
        // authors a lighter violet of its own rather than reusing it.
        assert_eq!(BEUI_LIGHT.gradient_accent.to, BEUI_LIGHT.violet);
        assert_ne!(BEUI_DARK.gradient_accent.to, BEUI_DARK.violet);
    }
}
