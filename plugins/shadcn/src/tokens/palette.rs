//! The vendored shadcn token tables: one [`ShadcnPalette`] per base preset per
//! brightness, transcribed from shadcn/ui's own registry.
//!
//! **Source:** `apps/v4/registry/themes.ts` of the shadcn/ui monorepo, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved **2026-08-17**.
//! Deliberately *not* the site's `apps/v4/app/globals.css`: its `:root` block
//! deviates from the registry presets (its light `--foreground`/`--primary` are
//! `oklch(0% 0 0)` where the registry `neutral` preset authors `0.145`/`0.205`),
//! and it adds site-level `surface`/code/selection tokens that are not part of
//! the theme contract a shadcn consumer gets. The registry preset is the
//! contract; this module ports it.
//!
//! # What a preset carries
//!
//! Each preset holds **31 color keys per brightness** — 18 scalar roles,
//! `chart-1..5`, and the 8-key `sidebar-*` group — plus (light only) the single
//! non-color `radius: 0.625rem`, which lives in
//! [`ShadcnRadius`](super::extension::ShadcnRadius) rather than here since it is
//! not a color. Every value in the source is a concrete OKLCH string with no
//! `var()` indirection, so a preset is self-contained.
//!
//! Seven base presets ship: `neutral`, `stone`, `zinc`, `mauve`, `olive`,
//! `mist`, `taupe` — identical in structure, differing only in hue/chroma. The
//! registry's 18 further *accent* themes (which override only
//! `primary`/`primary-foreground`, `chart-*`, and `sidebar-primary`) are a
//! recorded follow-on, not ported here.
//!
//! # OKLCH → sRGB conversion (authoring-time)
//!
//! The upstream values are CSS `oklch()`; frust's [`Color`] is sRGB. Converting
//! at *runtime* would mean either a color-science dependency or hand-rolled
//! matrix math on every theme build, so the conversion ran **once, at authoring
//! time**, and this module embeds the resulting 8-bit sRGB literals with the
//! original OKLCH string as a trailing comment on every line — the source of
//! truth stays visible next to the value it produced.
//!
//! The conversion is Björn Ottosson's reference Oklab pipeline (the same one
//! the CSS Color 4 specification's sample code implements):
//!
//! 1. `oklch(L C H)` → Oklab: `a = C·cos(H)`, `b = C·sin(H)` (H in radians).
//! 2. Oklab → LMS′ via the `M2⁻¹` matrix, then cube each component.
//! 3. LMS → linear sRGB via the `M1⁻¹` matrix.
//! 4. Clamp each linear channel into `[0, 1]`, apply the sRGB transfer function
//!    (`12.92·c` below `0.0031308`, else `1.055·c^(1/2.4) − 0.055`), and round
//!    to 8 bits.
//! 5. An `oklch(… / N%)` alpha form keeps its alpha as straight (un-premultiplied)
//!    alpha — `Color::from_rgba8`'s fourth argument.
//!
//! Step 4's clamp matters for exactly two values per preset: `destructive` is
//! authored outside the sRGB gamut in both brightnesses (it is Tailwind's
//! `red-600`/`red-400`, which are P3 colors), and clamping reproduces the
//! `#e7000b`/`#ff6467` sRGB fallbacks Tailwind itself publishes — the clamp is
//! the documented degrade, not a rounding artifact. Every clamped line is marked
//! `(out of sRGB gamut, clamped)` in its trailing comment.
//!
//! Spot-checking the achromatic ramps against Tailwind's published hexes
//! (`neutral-950` `#0a0a0a`, `neutral-100` `#f5f5f5`, `stone-500` `#79716b`, …)
//! confirms the pipeline reproduces upstream exactly.
//!
//! # Alpha tokens are kept as alpha
//!
//! Dark-mode `border` and `input` are authored as white washes
//! (`oklch(1 0 0 / 10%)`, `/ 15%`), not opaque greys. They stay translucent
//! here rather than being pre-flattened over a surface: shadcn's own borders
//! composite over whatever is behind them, and a component that strokes over a
//! non-surface backdrop is the only place where the two readings differ (see
//! `docs/WIDGETS_CODE_STANDARDS.md`'s alpha-border rule — it is a per-widget
//! call, and this catalog's call is "stay faithful to the source").

use frust::Color;

/// The 8 `sidebar-*` tokens of one preset/brightness, as their own group.
///
/// The sidebar component itself is a recorded follow-on (it needs the
/// cookie-backed collapsible shell shadcn's React version has), but its tokens
/// are part of the vendored preset, so they are ported with everything else and
/// reachable through [`ShadcnTokens`](super::extension::ShadcnTokens).
///
/// Field names drop the `sidebar-` prefix the CSS keys carry (`sidebar` itself
/// becomes `background`), since the struct name already says "sidebar".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadcnSidebar {
    /// `--sidebar`: the sidebar's own surface.
    pub background: Color,
    /// `--sidebar-foreground`: ink on that surface.
    pub foreground: Color,
    /// `--sidebar-primary`: the accent fill of a selected sidebar item.
    pub primary: Color,
    /// `--sidebar-primary-foreground`: ink on that accent fill.
    pub primary_foreground: Color,
    /// `--sidebar-accent`: the hover/selected wash.
    pub accent: Color,
    /// `--sidebar-accent-foreground`: ink on that wash.
    pub accent_foreground: Color,
    /// `--sidebar-border`: the sidebar's hairline rules.
    pub border: Color,
    /// `--sidebar-ring`: the sidebar's focus-ring color.
    pub ring: Color,
}

/// One shadcn base preset's token table for a **single brightness** — the
/// vendored CSS custom properties, one Rust field per key.
///
/// A preset is a pair of these ([`ShadcnBase::light`]/[`ShadcnBase::dark`]);
/// [`color_scheme`](super::theme::color_scheme) is what folds a pair onto
/// frust's 46-role `ColorScheme`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadcnPalette {
    /// `--background`: the app's base surface.
    pub background: Color,
    /// `--foreground`: default ink on `background`.
    pub foreground: Color,
    /// `--card`: a raised content panel's surface.
    pub card: Color,
    /// `--card-foreground`: ink on `card`.
    pub card_foreground: Color,
    /// `--popover`: an overlay panel's surface (identical to `card` in every
    /// shipped preset, kept separate because the source keeps it separate).
    pub popover: Color,
    /// `--popover-foreground`: ink on `popover`.
    pub popover_foreground: Color,
    /// `--primary`: the solid accent fill (a default button's background).
    pub primary: Color,
    /// `--primary-foreground`: ink on `primary`.
    pub primary_foreground: Color,
    /// `--secondary`: the low-emphasis filled surface.
    pub secondary: Color,
    /// `--secondary-foreground`: ink on `secondary`.
    pub secondary_foreground: Color,
    /// `--muted`: the muted surface (same value as `secondary`/`accent` in
    /// every shipped preset).
    pub muted: Color,
    /// `--muted-foreground`: the dimmed ink role.
    pub muted_foreground: Color,
    /// `--accent`: the hover/selected wash.
    pub accent: Color,
    /// `--accent-foreground`: ink on `accent`.
    pub accent_foreground: Color,
    /// `--destructive`: the danger role (no paired `-foreground` token — shadcn
    /// paints `text-white` over it, see
    /// [`color_scheme`](super::theme::color_scheme)).
    pub destructive: Color,
    /// `--border`: hairline rules and control borders.
    pub border: Color,
    /// `--input`: a form control's border (a touch stronger than `border` in
    /// dark mode: 15% vs 10% white).
    pub input: Color,
    /// `--ring`: the focus-ring color, painted at 50% opacity (see
    /// [`crate::style`]).
    pub ring: Color,
    /// `--chart-1` … `--chart-5`, in order.
    pub chart: [Color; 5],
    /// The `--sidebar-*` group.
    pub sidebar: ShadcnSidebar,
}

const NEUTRAL_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x0A, 0x0A, 0x0A), // oklch(0.145 0 0)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x0A, 0x0A, 0x0A), // oklch(0.145 0 0)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x0A, 0x0A, 0x0A), // oklch(0.145 0 0)
    primary: Color::from_rgb8(0x17, 0x17, 0x17),    // oklch(0.205 0 0)
    primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    secondary: Color::from_rgb8(0xF5, 0xF5, 0xF5),  // oklch(0.97 0 0)
    secondary_foreground: Color::from_rgb8(0x17, 0x17, 0x17), // oklch(0.205 0 0)
    muted: Color::from_rgb8(0xF5, 0xF5, 0xF5),      // oklch(0.97 0 0)
    muted_foreground: Color::from_rgb8(0x73, 0x73, 0x73), // oklch(0.556 0 0)
    accent: Color::from_rgb8(0xF5, 0xF5, 0xF5),     // oklch(0.97 0 0)
    accent_foreground: Color::from_rgb8(0x17, 0x17, 0x17), // oklch(0.205 0 0)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE5, 0xE5, 0xE5),      // oklch(0.922 0 0)
    input: Color::from_rgb8(0xE5, 0xE5, 0xE5),       // oklch(0.922 0 0)
    ring: Color::from_rgb8(0xA1, 0xA1, 0xA1),        // oklch(0.708 0 0)
    chart: [
        Color::from_rgb8(0xD4, 0xD4, 0xD4), // chart-1: oklch(0.87 0 0)
        Color::from_rgb8(0x73, 0x73, 0x73), // chart-2: oklch(0.556 0 0)
        Color::from_rgb8(0x52, 0x52, 0x52), // chart-3: oklch(0.439 0 0)
        Color::from_rgb8(0x40, 0x40, 0x40), // chart-4: oklch(0.371 0 0)
        Color::from_rgb8(0x26, 0x26, 0x26), // chart-5: oklch(0.269 0 0)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar: oklch(0.985 0 0)
        foreground: Color::from_rgb8(0x0A, 0x0A, 0x0A), // sidebar-foreground: oklch(0.145 0 0)
        primary: Color::from_rgb8(0x17, 0x17, 0x17),    // sidebar-primary: oklch(0.205 0 0)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-primary-foreground: oklch(0.985 0 0)
        accent: Color::from_rgb8(0xF5, 0xF5, 0xF5),             // sidebar-accent: oklch(0.97 0 0)
        accent_foreground: Color::from_rgb8(0x17, 0x17, 0x17), // sidebar-accent-foreground: oklch(0.205 0 0)
        border: Color::from_rgb8(0xE5, 0xE5, 0xE5),            // sidebar-border: oklch(0.922 0 0)
        ring: Color::from_rgb8(0xA1, 0xA1, 0xA1),              // sidebar-ring: oklch(0.708 0 0)
    },
};

const NEUTRAL_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x0A, 0x0A, 0x0A), // oklch(0.145 0 0)
    foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    card: Color::from_rgb8(0x17, 0x17, 0x17),       // oklch(0.205 0 0)
    card_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    popover: Color::from_rgb8(0x17, 0x17, 0x17),    // oklch(0.205 0 0)
    popover_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    primary: Color::from_rgb8(0xE5, 0xE5, 0xE5),    // oklch(0.922 0 0)
    primary_foreground: Color::from_rgb8(0x17, 0x17, 0x17), // oklch(0.205 0 0)
    secondary: Color::from_rgb8(0x26, 0x26, 0x26),  // oklch(0.269 0 0)
    secondary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    muted: Color::from_rgb8(0x26, 0x26, 0x26),      // oklch(0.269 0 0)
    muted_foreground: Color::from_rgb8(0xA1, 0xA1, 0xA1), // oklch(0.708 0 0)
    accent: Color::from_rgb8(0x26, 0x26, 0x26),     // oklch(0.269 0 0)
    accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x73, 0x73, 0x73),        // oklch(0.556 0 0)
    chart: [
        Color::from_rgb8(0xD4, 0xD4, 0xD4), // chart-1: oklch(0.87 0 0)
        Color::from_rgb8(0x73, 0x73, 0x73), // chart-2: oklch(0.556 0 0)
        Color::from_rgb8(0x52, 0x52, 0x52), // chart-3: oklch(0.439 0 0)
        Color::from_rgb8(0x40, 0x40, 0x40), // chart-4: oklch(0.371 0 0)
        Color::from_rgb8(0x26, 0x26, 0x26), // chart-5: oklch(0.269 0 0)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x17, 0x17, 0x17), // sidebar: oklch(0.205 0 0)
        foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-foreground: oklch(0.985 0 0)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-primary-foreground: oklch(0.985 0 0)
        accent: Color::from_rgb8(0x26, 0x26, 0x26),             // sidebar-accent: oklch(0.269 0 0)
        accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-accent-foreground: oklch(0.985 0 0)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x73, 0x73, 0x73),              // sidebar-ring: oklch(0.556 0 0)
    },
};

const STONE_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.25)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.25)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.25)
    primary: Color::from_rgb8(0x1C, 0x19, 0x17),    // oklch(0.216 0.006 56.043)
    primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // oklch(0.985 0.001 106.423)
    secondary: Color::from_rgb8(0xF5, 0xF5, 0xF4),  // oklch(0.97 0.001 106.424)
    secondary_foreground: Color::from_rgb8(0x1C, 0x19, 0x17), // oklch(0.216 0.006 56.043)
    muted: Color::from_rgb8(0xF5, 0xF5, 0xF4),      // oklch(0.97 0.001 106.424)
    muted_foreground: Color::from_rgb8(0x79, 0x71, 0x6B), // oklch(0.553 0.013 58.071)
    accent: Color::from_rgb8(0xF5, 0xF5, 0xF4),     // oklch(0.97 0.001 106.424)
    accent_foreground: Color::from_rgb8(0x1C, 0x19, 0x17), // oklch(0.216 0.006 56.043)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE7, 0xE5, 0xE4),      // oklch(0.923 0.003 48.717)
    input: Color::from_rgb8(0xE7, 0xE5, 0xE4),       // oklch(0.923 0.003 48.717)
    ring: Color::from_rgb8(0xA6, 0xA0, 0x9B),        // oklch(0.709 0.01 56.259)
    chart: [
        Color::from_rgb8(0xD6, 0xD3, 0xD1), // chart-1: oklch(0.869 0.005 56.366)
        Color::from_rgb8(0x79, 0x71, 0x6B), // chart-2: oklch(0.553 0.013 58.071)
        Color::from_rgb8(0x57, 0x53, 0x4D), // chart-3: oklch(0.444 0.011 73.639)
        Color::from_rgb8(0x44, 0x40, 0x3B), // chart-4: oklch(0.374 0.01 67.558)
        Color::from_rgb8(0x29, 0x25, 0x24), // chart-5: oklch(0.268 0.007 34.298)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xFA, 0xFA, 0xF9), // sidebar: oklch(0.985 0.001 106.423)
        foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // sidebar-foreground: oklch(0.147 0.004 49.25)
        primary: Color::from_rgb8(0x1C, 0x19, 0x17), // sidebar-primary: oklch(0.216 0.006 56.043)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // sidebar-primary-foreground: oklch(0.985 0.001 106.423)
        accent: Color::from_rgb8(0xF5, 0xF5, 0xF4), // sidebar-accent: oklch(0.97 0.001 106.424)
        accent_foreground: Color::from_rgb8(0x1C, 0x19, 0x17), // sidebar-accent-foreground: oklch(0.216 0.006 56.043)
        border: Color::from_rgb8(0xE7, 0xE5, 0xE4), // sidebar-border: oklch(0.923 0.003 48.717)
        ring: Color::from_rgb8(0xA6, 0xA0, 0x9B),   // sidebar-ring: oklch(0.709 0.01 56.259)
    },
};

const STONE_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.25)
    foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // oklch(0.985 0.001 106.423)
    card: Color::from_rgb8(0x1C, 0x19, 0x17),       // oklch(0.216 0.006 56.043)
    card_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // oklch(0.985 0.001 106.423)
    popover: Color::from_rgb8(0x1C, 0x19, 0x17),    // oklch(0.216 0.006 56.043)
    popover_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // oklch(0.985 0.001 106.423)
    primary: Color::from_rgb8(0xE7, 0xE5, 0xE4),    // oklch(0.923 0.003 48.717)
    primary_foreground: Color::from_rgb8(0x1C, 0x19, 0x17), // oklch(0.216 0.006 56.043)
    secondary: Color::from_rgb8(0x29, 0x25, 0x24),  // oklch(0.268 0.007 34.298)
    secondary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // oklch(0.985 0.001 106.423)
    muted: Color::from_rgb8(0x29, 0x25, 0x24),      // oklch(0.268 0.007 34.298)
    muted_foreground: Color::from_rgb8(0xA6, 0xA0, 0x9B), // oklch(0.709 0.01 56.259)
    accent: Color::from_rgb8(0x29, 0x25, 0x24),     // oklch(0.268 0.007 34.298)
    accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // oklch(0.985 0.001 106.423)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x79, 0x71, 0x6B),        // oklch(0.553 0.013 58.071)
    chart: [
        Color::from_rgb8(0xD6, 0xD3, 0xD1), // chart-1: oklch(0.869 0.005 56.366)
        Color::from_rgb8(0x79, 0x71, 0x6B), // chart-2: oklch(0.553 0.013 58.071)
        Color::from_rgb8(0x57, 0x53, 0x4D), // chart-3: oklch(0.444 0.011 73.639)
        Color::from_rgb8(0x44, 0x40, 0x3B), // chart-4: oklch(0.374 0.01 67.558)
        Color::from_rgb8(0x29, 0x25, 0x24), // chart-5: oklch(0.268 0.007 34.298)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x1C, 0x19, 0x17), // sidebar: oklch(0.216 0.006 56.043)
        foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // sidebar-foreground: oklch(0.985 0.001 106.423)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // sidebar-primary-foreground: oklch(0.985 0.001 106.423)
        accent: Color::from_rgb8(0x29, 0x25, 0x24), // sidebar-accent: oklch(0.268 0.007 34.298)
        accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xF9), // sidebar-accent-foreground: oklch(0.985 0.001 106.423)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x79, 0x71, 0x6B), // sidebar-ring: oklch(0.553 0.013 58.071)
    },
};

const ZINC_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x09, 0x09, 0x0B), // oklch(0.141 0.005 285.823)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x09, 0x09, 0x0B), // oklch(0.141 0.005 285.823)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x09, 0x09, 0x0B), // oklch(0.141 0.005 285.823)
    primary: Color::from_rgb8(0x18, 0x18, 0x1B),    // oklch(0.21 0.006 285.885)
    primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    secondary: Color::from_rgb8(0xF4, 0xF4, 0xF5),  // oklch(0.967 0.001 286.375)
    secondary_foreground: Color::from_rgb8(0x18, 0x18, 0x1B), // oklch(0.21 0.006 285.885)
    muted: Color::from_rgb8(0xF4, 0xF4, 0xF5),      // oklch(0.967 0.001 286.375)
    muted_foreground: Color::from_rgb8(0x71, 0x71, 0x7B), // oklch(0.552 0.016 285.938)
    accent: Color::from_rgb8(0xF4, 0xF4, 0xF5),     // oklch(0.967 0.001 286.375)
    accent_foreground: Color::from_rgb8(0x18, 0x18, 0x1B), // oklch(0.21 0.006 285.885)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE4, 0xE4, 0xE7),      // oklch(0.92 0.004 286.32)
    input: Color::from_rgb8(0xE4, 0xE4, 0xE7),       // oklch(0.92 0.004 286.32)
    ring: Color::from_rgb8(0x9F, 0x9F, 0xA9),        // oklch(0.705 0.015 286.067)
    chart: [
        Color::from_rgb8(0xD4, 0xD4, 0xD8), // chart-1: oklch(0.871 0.006 286.286)
        Color::from_rgb8(0x71, 0x71, 0x7B), // chart-2: oklch(0.552 0.016 285.938)
        Color::from_rgb8(0x52, 0x52, 0x5C), // chart-3: oklch(0.442 0.017 285.786)
        Color::from_rgb8(0x3F, 0x3F, 0x46), // chart-4: oklch(0.37 0.013 285.805)
        Color::from_rgb8(0x27, 0x27, 0x2A), // chart-5: oklch(0.274 0.006 286.033)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar: oklch(0.985 0 0)
        foreground: Color::from_rgb8(0x09, 0x09, 0x0B), // sidebar-foreground: oklch(0.141 0.005 285.823)
        primary: Color::from_rgb8(0x18, 0x18, 0x1B), // sidebar-primary: oklch(0.21 0.006 285.885)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-primary-foreground: oklch(0.985 0 0)
        accent: Color::from_rgb8(0xF4, 0xF4, 0xF5), // sidebar-accent: oklch(0.967 0.001 286.375)
        accent_foreground: Color::from_rgb8(0x18, 0x18, 0x1B), // sidebar-accent-foreground: oklch(0.21 0.006 285.885)
        border: Color::from_rgb8(0xE4, 0xE4, 0xE7), // sidebar-border: oklch(0.92 0.004 286.32)
        ring: Color::from_rgb8(0x9F, 0x9F, 0xA9),   // sidebar-ring: oklch(0.705 0.015 286.067)
    },
};

const ZINC_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x09, 0x09, 0x0B), // oklch(0.141 0.005 285.823)
    foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    card: Color::from_rgb8(0x18, 0x18, 0x1B),       // oklch(0.21 0.006 285.885)
    card_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    popover: Color::from_rgb8(0x18, 0x18, 0x1B),    // oklch(0.21 0.006 285.885)
    popover_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    primary: Color::from_rgb8(0xE4, 0xE4, 0xE7),    // oklch(0.92 0.004 286.32)
    primary_foreground: Color::from_rgb8(0x18, 0x18, 0x1B), // oklch(0.21 0.006 285.885)
    secondary: Color::from_rgb8(0x27, 0x27, 0x2A),  // oklch(0.274 0.006 286.033)
    secondary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    muted: Color::from_rgb8(0x27, 0x27, 0x2A),      // oklch(0.274 0.006 286.033)
    muted_foreground: Color::from_rgb8(0x9F, 0x9F, 0xA9), // oklch(0.705 0.015 286.067)
    accent: Color::from_rgb8(0x27, 0x27, 0x2A),     // oklch(0.274 0.006 286.033)
    accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x71, 0x71, 0x7B),        // oklch(0.552 0.016 285.938)
    chart: [
        Color::from_rgb8(0xD4, 0xD4, 0xD8), // chart-1: oklch(0.871 0.006 286.286)
        Color::from_rgb8(0x71, 0x71, 0x7B), // chart-2: oklch(0.552 0.016 285.938)
        Color::from_rgb8(0x52, 0x52, 0x5C), // chart-3: oklch(0.442 0.017 285.786)
        Color::from_rgb8(0x3F, 0x3F, 0x46), // chart-4: oklch(0.37 0.013 285.805)
        Color::from_rgb8(0x27, 0x27, 0x2A), // chart-5: oklch(0.274 0.006 286.033)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x18, 0x18, 0x1B), // sidebar: oklch(0.21 0.006 285.885)
        foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-foreground: oklch(0.985 0 0)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-primary-foreground: oklch(0.985 0 0)
        accent: Color::from_rgb8(0x27, 0x27, 0x2A), // sidebar-accent: oklch(0.274 0.006 286.033)
        accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-accent-foreground: oklch(0.985 0 0)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x71, 0x71, 0x7B), // sidebar-ring: oklch(0.552 0.016 285.938)
    },
};

const MAUVE_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x0C, 0x09, 0x0C), // oklch(0.145 0.008 326)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x0C, 0x09, 0x0C), // oklch(0.145 0.008 326)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x0C, 0x09, 0x0C), // oklch(0.145 0.008 326)
    primary: Color::from_rgb8(0x1D, 0x16, 0x1E),    // oklch(0.212 0.019 322.12)
    primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    secondary: Color::from_rgb8(0xF3, 0xF1, 0xF3),  // oklch(0.96 0.003 325.6)
    secondary_foreground: Color::from_rgb8(0x1D, 0x16, 0x1E), // oklch(0.212 0.019 322.12)
    muted: Color::from_rgb8(0xF3, 0xF1, 0xF3),      // oklch(0.96 0.003 325.6)
    muted_foreground: Color::from_rgb8(0x79, 0x69, 0x7B), // oklch(0.542 0.034 322.5)
    accent: Color::from_rgb8(0xF3, 0xF1, 0xF3),     // oklch(0.96 0.003 325.6)
    accent_foreground: Color::from_rgb8(0x1D, 0x16, 0x1E), // oklch(0.212 0.019 322.12)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE7, 0xE4, 0xE7),      // oklch(0.922 0.005 325.62)
    input: Color::from_rgb8(0xE7, 0xE4, 0xE7),       // oklch(0.922 0.005 325.62)
    ring: Color::from_rgb8(0xA8, 0x9E, 0xA9),        // oklch(0.711 0.019 323.02)
    chart: [
        Color::from_rgb8(0xD7, 0xD0, 0xD7), // chart-1: oklch(0.865 0.012 325.68)
        Color::from_rgb8(0x79, 0x69, 0x7B), // chart-2: oklch(0.542 0.034 322.5)
        Color::from_rgb8(0x59, 0x4C, 0x5B), // chart-3: oklch(0.435 0.029 321.78)
        Color::from_rgb8(0x46, 0x39, 0x47), // chart-4: oklch(0.364 0.029 323.89)
        Color::from_rgb8(0x2A, 0x21, 0x2C), // chart-5: oklch(0.263 0.024 320.12)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar: oklch(0.985 0 0)
        foreground: Color::from_rgb8(0x0C, 0x09, 0x0C), // sidebar-foreground: oklch(0.145 0.008 326)
        primary: Color::from_rgb8(0x1D, 0x16, 0x1E), // sidebar-primary: oklch(0.212 0.019 322.12)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-primary-foreground: oklch(0.985 0 0)
        accent: Color::from_rgb8(0xF3, 0xF1, 0xF3), // sidebar-accent: oklch(0.96 0.003 325.6)
        accent_foreground: Color::from_rgb8(0x1D, 0x16, 0x1E), // sidebar-accent-foreground: oklch(0.212 0.019 322.12)
        border: Color::from_rgb8(0xE7, 0xE4, 0xE7), // sidebar-border: oklch(0.922 0.005 325.62)
        ring: Color::from_rgb8(0xA8, 0x9E, 0xA9),   // sidebar-ring: oklch(0.711 0.019 323.02)
    },
};

const MAUVE_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x0C, 0x09, 0x0C), // oklch(0.145 0.008 326)
    foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    card: Color::from_rgb8(0x1D, 0x16, 0x1E),       // oklch(0.212 0.019 322.12)
    card_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    popover: Color::from_rgb8(0x1D, 0x16, 0x1E),    // oklch(0.212 0.019 322.12)
    popover_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    primary: Color::from_rgb8(0xE7, 0xE4, 0xE7),    // oklch(0.922 0.005 325.62)
    primary_foreground: Color::from_rgb8(0x1D, 0x16, 0x1E), // oklch(0.212 0.019 322.12)
    secondary: Color::from_rgb8(0x2A, 0x21, 0x2C),  // oklch(0.263 0.024 320.12)
    secondary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    muted: Color::from_rgb8(0x2A, 0x21, 0x2C),      // oklch(0.263 0.024 320.12)
    muted_foreground: Color::from_rgb8(0xA8, 0x9E, 0xA9), // oklch(0.711 0.019 323.02)
    accent: Color::from_rgb8(0x2A, 0x21, 0x2C),     // oklch(0.263 0.024 320.12)
    accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // oklch(0.985 0 0)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x79, 0x69, 0x7B),        // oklch(0.542 0.034 322.5)
    chart: [
        Color::from_rgb8(0xD7, 0xD0, 0xD7), // chart-1: oklch(0.865 0.012 325.68)
        Color::from_rgb8(0x79, 0x69, 0x7B), // chart-2: oklch(0.542 0.034 322.5)
        Color::from_rgb8(0x59, 0x4C, 0x5B), // chart-3: oklch(0.435 0.029 321.78)
        Color::from_rgb8(0x46, 0x39, 0x47), // chart-4: oklch(0.364 0.029 323.89)
        Color::from_rgb8(0x2A, 0x21, 0x2C), // chart-5: oklch(0.263 0.024 320.12)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x1D, 0x16, 0x1E), // sidebar: oklch(0.212 0.019 322.12)
        foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-foreground: oklch(0.985 0 0)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-primary-foreground: oklch(0.985 0 0)
        accent: Color::from_rgb8(0x2A, 0x21, 0x2C), // sidebar-accent: oklch(0.263 0.024 320.12)
        accent_foreground: Color::from_rgb8(0xFA, 0xFA, 0xFA), // sidebar-accent-foreground: oklch(0.985 0 0)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x79, 0x69, 0x7B), // sidebar-ring: oklch(0.542 0.034 322.5)
    },
};

const OLIVE_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x0C, 0x0C, 0x09), // oklch(0.153 0.006 107.1)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x0C, 0x0C, 0x09), // oklch(0.153 0.006 107.1)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x0C, 0x0C, 0x09), // oklch(0.153 0.006 107.1)
    primary: Color::from_rgb8(0x1D, 0x1D, 0x16),    // oklch(0.228 0.013 107.4)
    primary_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // oklch(0.988 0.003 106.5)
    secondary: Color::from_rgb8(0xF4, 0xF4, 0xF0),  // oklch(0.966 0.005 106.5)
    secondary_foreground: Color::from_rgb8(0x1D, 0x1D, 0x16), // oklch(0.228 0.013 107.4)
    muted: Color::from_rgb8(0xF4, 0xF4, 0xF0),      // oklch(0.966 0.005 106.5)
    muted_foreground: Color::from_rgb8(0x7C, 0x7C, 0x67), // oklch(0.58 0.031 107.3)
    accent: Color::from_rgb8(0xF4, 0xF4, 0xF0),     // oklch(0.966 0.005 106.5)
    accent_foreground: Color::from_rgb8(0x1D, 0x1D, 0x16), // oklch(0.228 0.013 107.4)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE8, 0xE8, 0xE3),      // oklch(0.93 0.007 106.5)
    input: Color::from_rgb8(0xE8, 0xE8, 0xE3),       // oklch(0.93 0.007 106.5)
    ring: Color::from_rgb8(0xAB, 0xAB, 0x9C),        // oklch(0.737 0.021 106.9)
    chart: [
        Color::from_rgb8(0xD8, 0xD8, 0xD0), // chart-1: oklch(0.88 0.011 106.6)
        Color::from_rgb8(0x7C, 0x7C, 0x67), // chart-2: oklch(0.58 0.031 107.3)
        Color::from_rgb8(0x5B, 0x5B, 0x4B), // chart-3: oklch(0.466 0.025 107.3)
        Color::from_rgb8(0x47, 0x47, 0x39), // chart-4: oklch(0.394 0.023 107.4)
        Color::from_rgb8(0x2B, 0x2B, 0x22), // chart-5: oklch(0.286 0.016 107.4)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xFB, 0xFB, 0xF9), // sidebar: oklch(0.988 0.003 106.5)
        foreground: Color::from_rgb8(0x0C, 0x0C, 0x09), // sidebar-foreground: oklch(0.153 0.006 107.1)
        primary: Color::from_rgb8(0x1D, 0x1D, 0x16),    // sidebar-primary: oklch(0.228 0.013 107.4)
        primary_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // sidebar-primary-foreground: oklch(0.988 0.003 106.5)
        accent: Color::from_rgb8(0xF4, 0xF4, 0xF0), // sidebar-accent: oklch(0.966 0.005 106.5)
        accent_foreground: Color::from_rgb8(0x1D, 0x1D, 0x16), // sidebar-accent-foreground: oklch(0.228 0.013 107.4)
        border: Color::from_rgb8(0xE8, 0xE8, 0xE3), // sidebar-border: oklch(0.93 0.007 106.5)
        ring: Color::from_rgb8(0xAB, 0xAB, 0x9C),   // sidebar-ring: oklch(0.737 0.021 106.9)
    },
};

const OLIVE_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x0C, 0x0C, 0x09), // oklch(0.153 0.006 107.1)
    foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // oklch(0.988 0.003 106.5)
    card: Color::from_rgb8(0x1D, 0x1D, 0x16),       // oklch(0.228 0.013 107.4)
    card_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // oklch(0.988 0.003 106.5)
    popover: Color::from_rgb8(0x1D, 0x1D, 0x16),    // oklch(0.228 0.013 107.4)
    popover_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // oklch(0.988 0.003 106.5)
    primary: Color::from_rgb8(0xE8, 0xE8, 0xE3),    // oklch(0.93 0.007 106.5)
    primary_foreground: Color::from_rgb8(0x1D, 0x1D, 0x16), // oklch(0.228 0.013 107.4)
    secondary: Color::from_rgb8(0x2B, 0x2B, 0x22),  // oklch(0.286 0.016 107.4)
    secondary_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // oklch(0.988 0.003 106.5)
    muted: Color::from_rgb8(0x2B, 0x2B, 0x22),      // oklch(0.286 0.016 107.4)
    muted_foreground: Color::from_rgb8(0xAB, 0xAB, 0x9C), // oklch(0.737 0.021 106.9)
    accent: Color::from_rgb8(0x2B, 0x2B, 0x22),     // oklch(0.286 0.016 107.4)
    accent_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // oklch(0.988 0.003 106.5)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x7C, 0x7C, 0x67),        // oklch(0.58 0.031 107.3)
    chart: [
        Color::from_rgb8(0xD8, 0xD8, 0xD0), // chart-1: oklch(0.88 0.011 106.6)
        Color::from_rgb8(0x7C, 0x7C, 0x67), // chart-2: oklch(0.58 0.031 107.3)
        Color::from_rgb8(0x5B, 0x5B, 0x4B), // chart-3: oklch(0.466 0.025 107.3)
        Color::from_rgb8(0x47, 0x47, 0x39), // chart-4: oklch(0.394 0.023 107.4)
        Color::from_rgb8(0x2B, 0x2B, 0x22), // chart-5: oklch(0.286 0.016 107.4)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x1D, 0x1D, 0x16), // sidebar: oklch(0.228 0.013 107.4)
        foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // sidebar-foreground: oklch(0.988 0.003 106.5)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // sidebar-primary-foreground: oklch(0.988 0.003 106.5)
        accent: Color::from_rgb8(0x2B, 0x2B, 0x22), // sidebar-accent: oklch(0.286 0.016 107.4)
        accent_foreground: Color::from_rgb8(0xFB, 0xFB, 0xF9), // sidebar-accent-foreground: oklch(0.988 0.003 106.5)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x7C, 0x7C, 0x67), // sidebar-ring: oklch(0.58 0.031 107.3)
    },
};

const MIST_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x09, 0x0B, 0x0C), // oklch(0.148 0.004 228.8)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x09, 0x0B, 0x0C), // oklch(0.148 0.004 228.8)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x09, 0x0B, 0x0C), // oklch(0.148 0.004 228.8)
    primary: Color::from_rgb8(0x16, 0x1B, 0x1D),    // oklch(0.218 0.008 223.9)
    primary_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // oklch(0.987 0.002 197.1)
    secondary: Color::from_rgb8(0xF1, 0xF3, 0xF3),  // oklch(0.963 0.002 197.1)
    secondary_foreground: Color::from_rgb8(0x16, 0x1B, 0x1D), // oklch(0.218 0.008 223.9)
    muted: Color::from_rgb8(0xF1, 0xF3, 0xF3),      // oklch(0.963 0.002 197.1)
    muted_foreground: Color::from_rgb8(0x67, 0x78, 0x7C), // oklch(0.56 0.021 213.5)
    accent: Color::from_rgb8(0xF1, 0xF3, 0xF3),     // oklch(0.963 0.002 197.1)
    accent_foreground: Color::from_rgb8(0x16, 0x1B, 0x1D), // oklch(0.218 0.008 223.9)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE3, 0xE7, 0xE8),      // oklch(0.925 0.005 214.3)
    input: Color::from_rgb8(0xE3, 0xE7, 0xE8),       // oklch(0.925 0.005 214.3)
    ring: Color::from_rgb8(0x9C, 0xA8, 0xAB),        // oklch(0.723 0.014 214.4)
    chart: [
        Color::from_rgb8(0xD0, 0xD6, 0xD8), // chart-1: oklch(0.872 0.007 219.6)
        Color::from_rgb8(0x67, 0x78, 0x7C), // chart-2: oklch(0.56 0.021 213.5)
        Color::from_rgb8(0x4B, 0x58, 0x5B), // chart-3: oklch(0.45 0.017 213.2)
        Color::from_rgb8(0x39, 0x44, 0x47), // chart-4: oklch(0.378 0.015 216)
        Color::from_rgb8(0x22, 0x29, 0x2B), // chart-5: oklch(0.275 0.011 216.9)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xF9, 0xFB, 0xFB), // sidebar: oklch(0.987 0.002 197.1)
        foreground: Color::from_rgb8(0x09, 0x0B, 0x0C), // sidebar-foreground: oklch(0.148 0.004 228.8)
        primary: Color::from_rgb8(0x16, 0x1B, 0x1D),    // sidebar-primary: oklch(0.218 0.008 223.9)
        primary_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // sidebar-primary-foreground: oklch(0.987 0.002 197.1)
        accent: Color::from_rgb8(0xF1, 0xF3, 0xF3), // sidebar-accent: oklch(0.963 0.002 197.1)
        accent_foreground: Color::from_rgb8(0x16, 0x1B, 0x1D), // sidebar-accent-foreground: oklch(0.218 0.008 223.9)
        border: Color::from_rgb8(0xE3, 0xE7, 0xE8), // sidebar-border: oklch(0.925 0.005 214.3)
        ring: Color::from_rgb8(0x9C, 0xA8, 0xAB),   // sidebar-ring: oklch(0.723 0.014 214.4)
    },
};

const MIST_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x09, 0x0B, 0x0C), // oklch(0.148 0.004 228.8)
    foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // oklch(0.987 0.002 197.1)
    card: Color::from_rgb8(0x16, 0x1B, 0x1D),       // oklch(0.218 0.008 223.9)
    card_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // oklch(0.987 0.002 197.1)
    popover: Color::from_rgb8(0x16, 0x1B, 0x1D),    // oklch(0.218 0.008 223.9)
    popover_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // oklch(0.987 0.002 197.1)
    primary: Color::from_rgb8(0xE3, 0xE7, 0xE8),    // oklch(0.925 0.005 214.3)
    primary_foreground: Color::from_rgb8(0x16, 0x1B, 0x1D), // oklch(0.218 0.008 223.9)
    secondary: Color::from_rgb8(0x22, 0x29, 0x2B),  // oklch(0.275 0.011 216.9)
    secondary_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // oklch(0.987 0.002 197.1)
    muted: Color::from_rgb8(0x22, 0x29, 0x2B),      // oklch(0.275 0.011 216.9)
    muted_foreground: Color::from_rgb8(0x9C, 0xA8, 0xAB), // oklch(0.723 0.014 214.4)
    accent: Color::from_rgb8(0x22, 0x29, 0x2B),     // oklch(0.275 0.011 216.9)
    accent_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // oklch(0.987 0.002 197.1)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x67, 0x78, 0x7C),        // oklch(0.56 0.021 213.5)
    chart: [
        Color::from_rgb8(0xD0, 0xD6, 0xD8), // chart-1: oklch(0.872 0.007 219.6)
        Color::from_rgb8(0x67, 0x78, 0x7C), // chart-2: oklch(0.56 0.021 213.5)
        Color::from_rgb8(0x4B, 0x58, 0x5B), // chart-3: oklch(0.45 0.017 213.2)
        Color::from_rgb8(0x39, 0x44, 0x47), // chart-4: oklch(0.378 0.015 216)
        Color::from_rgb8(0x22, 0x29, 0x2B), // chart-5: oklch(0.275 0.011 216.9)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x16, 0x1B, 0x1D), // sidebar: oklch(0.218 0.008 223.9)
        foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // sidebar-foreground: oklch(0.987 0.002 197.1)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // sidebar-primary-foreground: oklch(0.987 0.002 197.1)
        accent: Color::from_rgb8(0x22, 0x29, 0x2B), // sidebar-accent: oklch(0.275 0.011 216.9)
        accent_foreground: Color::from_rgb8(0xF9, 0xFB, 0xFB), // sidebar-accent-foreground: oklch(0.987 0.002 197.1)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x67, 0x78, 0x7C), // sidebar-ring: oklch(0.56 0.021 213.5)
    },
};

const TAUPE_LIGHT: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0xFF, 0xFF, 0xFF), // oklch(1 0 0)
    foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.3)
    card: Color::from_rgb8(0xFF, 0xFF, 0xFF),       // oklch(1 0 0)
    card_foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.3)
    popover: Color::from_rgb8(0xFF, 0xFF, 0xFF),    // oklch(1 0 0)
    popover_foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.3)
    primary: Color::from_rgb8(0x1D, 0x18, 0x16),    // oklch(0.214 0.009 43.1)
    primary_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // oklch(0.986 0.002 67.8)
    secondary: Color::from_rgb8(0xF3, 0xF1, 0xF1),  // oklch(0.96 0.002 17.2)
    secondary_foreground: Color::from_rgb8(0x1D, 0x18, 0x16), // oklch(0.214 0.009 43.1)
    muted: Color::from_rgb8(0xF3, 0xF1, 0xF1),      // oklch(0.96 0.002 17.2)
    muted_foreground: Color::from_rgb8(0x7C, 0x6D, 0x67), // oklch(0.547 0.021 43.1)
    accent: Color::from_rgb8(0xF3, 0xF1, 0xF1),     // oklch(0.96 0.002 17.2)
    accent_foreground: Color::from_rgb8(0x1D, 0x18, 0x16), // oklch(0.214 0.009 43.1)
    destructive: Color::from_rgb8(0xE7, 0x00, 0x0B), // oklch(0.577 0.245 27.325) (out of sRGB gamut, clamped)
    border: Color::from_rgb8(0xE8, 0xE4, 0xE3),      // oklch(0.922 0.005 34.3)
    input: Color::from_rgb8(0xE8, 0xE4, 0xE3),       // oklch(0.922 0.005 34.3)
    ring: Color::from_rgb8(0xAB, 0xA0, 0x9C),        // oklch(0.714 0.014 41.2)
    chart: [
        Color::from_rgb8(0xD8, 0xD2, 0xD0), // chart-1: oklch(0.868 0.007 39.5)
        Color::from_rgb8(0x7C, 0x6D, 0x67), // chart-2: oklch(0.547 0.021 43.1)
        Color::from_rgb8(0x5B, 0x4F, 0x4B), // chart-3: oklch(0.438 0.017 39.3)
        Color::from_rgb8(0x47, 0x3C, 0x39), // chart-4: oklch(0.367 0.016 35.7)
        Color::from_rgb8(0x2B, 0x24, 0x22), // chart-5: oklch(0.268 0.011 36.5)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0xFB, 0xFA, 0xF9), // sidebar: oklch(0.986 0.002 67.8)
        foreground: Color::from_rgb8(0x0C, 0x0A, 0x09), // sidebar-foreground: oklch(0.147 0.004 49.3)
        primary: Color::from_rgb8(0x1D, 0x18, 0x16),    // sidebar-primary: oklch(0.214 0.009 43.1)
        primary_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // sidebar-primary-foreground: oklch(0.986 0.002 67.8)
        accent: Color::from_rgb8(0xF3, 0xF1, 0xF1), // sidebar-accent: oklch(0.96 0.002 17.2)
        accent_foreground: Color::from_rgb8(0x1D, 0x18, 0x16), // sidebar-accent-foreground: oklch(0.214 0.009 43.1)
        border: Color::from_rgb8(0xE8, 0xE4, 0xE3), // sidebar-border: oklch(0.922 0.005 34.3)
        ring: Color::from_rgb8(0xAB, 0xA0, 0x9C),   // sidebar-ring: oklch(0.714 0.014 41.2)
    },
};

const TAUPE_DARK: ShadcnPalette = ShadcnPalette {
    background: Color::from_rgb8(0x0C, 0x0A, 0x09), // oklch(0.147 0.004 49.3)
    foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // oklch(0.986 0.002 67.8)
    card: Color::from_rgb8(0x1D, 0x18, 0x16),       // oklch(0.214 0.009 43.1)
    card_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // oklch(0.986 0.002 67.8)
    popover: Color::from_rgb8(0x1D, 0x18, 0x16),    // oklch(0.214 0.009 43.1)
    popover_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // oklch(0.986 0.002 67.8)
    primary: Color::from_rgb8(0xE8, 0xE4, 0xE3),    // oklch(0.922 0.005 34.3)
    primary_foreground: Color::from_rgb8(0x1D, 0x18, 0x16), // oklch(0.214 0.009 43.1)
    secondary: Color::from_rgb8(0x2B, 0x24, 0x22),  // oklch(0.268 0.011 36.5)
    secondary_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // oklch(0.986 0.002 67.8)
    muted: Color::from_rgb8(0x2B, 0x24, 0x22),      // oklch(0.268 0.011 36.5)
    muted_foreground: Color::from_rgb8(0xAB, 0xA0, 0x9C), // oklch(0.714 0.014 41.2)
    accent: Color::from_rgb8(0x2B, 0x24, 0x22),     // oklch(0.268 0.011 36.5)
    accent_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // oklch(0.986 0.002 67.8)
    destructive: Color::from_rgb8(0xFF, 0x64, 0x67), // oklch(0.704 0.191 22.216) (out of sRGB gamut, clamped)
    border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A), // oklch(1 0 0 / 10%)
    input: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26), // oklch(1 0 0 / 15%)
    ring: Color::from_rgb8(0x7C, 0x6D, 0x67),        // oklch(0.547 0.021 43.1)
    chart: [
        Color::from_rgb8(0xD8, 0xD2, 0xD0), // chart-1: oklch(0.868 0.007 39.5)
        Color::from_rgb8(0x7C, 0x6D, 0x67), // chart-2: oklch(0.547 0.021 43.1)
        Color::from_rgb8(0x5B, 0x4F, 0x4B), // chart-3: oklch(0.438 0.017 39.3)
        Color::from_rgb8(0x47, 0x3C, 0x39), // chart-4: oklch(0.367 0.016 35.7)
        Color::from_rgb8(0x2B, 0x24, 0x22), // chart-5: oklch(0.268 0.011 36.5)
    ],
    sidebar: ShadcnSidebar {
        background: Color::from_rgb8(0x1D, 0x18, 0x16), // sidebar: oklch(0.214 0.009 43.1)
        foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // sidebar-foreground: oklch(0.986 0.002 67.8)
        primary: Color::from_rgb8(0x14, 0x47, 0xE6), // sidebar-primary: oklch(0.488 0.243 264.376)
        primary_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // sidebar-primary-foreground: oklch(0.986 0.002 67.8)
        accent: Color::from_rgb8(0x2B, 0x24, 0x22), // sidebar-accent: oklch(0.268 0.011 36.5)
        accent_foreground: Color::from_rgb8(0xFB, 0xFA, 0xF9), // sidebar-accent-foreground: oklch(0.986 0.002 67.8)
        border: Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1A),     // sidebar-border: oklch(1 0 0 / 10%)
        ring: Color::from_rgb8(0x7C, 0x6D, 0x67), // sidebar-ring: oklch(0.547 0.021 43.1)
    },
};

/// Which shadcn **base** preset a theme is built from.
///
/// The seven base presets differ only in hue/chroma — `Neutral` is the
/// achromatic one shadcn's docs default to, and the one
/// [`theme()`](fn@super::theme::theme) builds. Every base carries the identical key
/// set, so a consumer can swap bases without any component knowing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ShadcnBase {
    /// `neutral` — achromatic (hue 0°). shadcn's own default.
    #[default]
    Neutral,
    /// `stone` — warm grey (hue ≈49.25°).
    Stone,
    /// `zinc` — cool grey (hue ≈285.8°).
    Zinc,
    /// `mauve` — pink-leaning grey (hue ≈326°).
    Mauve,
    /// `olive` — green-leaning grey (hue ≈107.1°).
    Olive,
    /// `mist` — blue-leaning grey (hue ≈228.8°).
    Mist,
    /// `taupe` — brown-leaning grey (hue ≈49.3°).
    Taupe,
}

impl ShadcnBase {
    /// Every base preset, in the order the registry lists them.
    pub const ALL: [ShadcnBase; 7] = [
        ShadcnBase::Neutral,
        ShadcnBase::Stone,
        ShadcnBase::Zinc,
        ShadcnBase::Mauve,
        ShadcnBase::Olive,
        ShadcnBase::Mist,
        ShadcnBase::Taupe,
    ];

    /// The preset's upstream registry `name` (`"neutral"`, `"stone"`, …) — the
    /// id a shadcn user would name in `components.json`.
    pub const fn id(self) -> &'static str {
        match self {
            ShadcnBase::Neutral => "neutral",
            ShadcnBase::Stone => "stone",
            ShadcnBase::Zinc => "zinc",
            ShadcnBase::Mauve => "mauve",
            ShadcnBase::Olive => "olive",
            ShadcnBase::Mist => "mist",
            ShadcnBase::Taupe => "taupe",
        }
    }

    /// This preset's light-mode token table.
    pub const fn light(self) -> ShadcnPalette {
        match self {
            ShadcnBase::Neutral => NEUTRAL_LIGHT,
            ShadcnBase::Stone => STONE_LIGHT,
            ShadcnBase::Zinc => ZINC_LIGHT,
            ShadcnBase::Mauve => MAUVE_LIGHT,
            ShadcnBase::Olive => OLIVE_LIGHT,
            ShadcnBase::Mist => MIST_LIGHT,
            ShadcnBase::Taupe => TAUPE_LIGHT,
        }
    }

    /// This preset's dark-mode token table.
    pub const fn dark(self) -> ShadcnPalette {
        match self {
            ShadcnBase::Neutral => NEUTRAL_DARK,
            ShadcnBase::Stone => STONE_DARK,
            ShadcnBase::Zinc => ZINC_DARK,
            ShadcnBase::Mauve => MAUVE_DARK,
            ShadcnBase::Olive => OLIVE_DARK,
            ShadcnBase::Mist => MIST_DARK,
            ShadcnBase::Taupe => TAUPE_DARK,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_light_spot_values_match_the_vendored_preset() {
        let p = ShadcnBase::Neutral.light();
        // oklch(1 0 0) / oklch(0.145 0 0) / oklch(0.205 0 0) / oklch(0.922 0 0)
        assert_eq!(p.background, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(p.foreground, Color::from_rgb8(0x0A, 0x0A, 0x0A));
        assert_eq!(p.primary, Color::from_rgb8(0x17, 0x17, 0x17));
        assert_eq!(p.border, Color::from_rgb8(0xE5, 0xE5, 0xE5));
        // Tailwind's own published sRGB fallback for `red-600`.
        assert_eq!(p.destructive, Color::from_rgb8(0xE7, 0x00, 0x0B));
    }

    #[test]
    fn neutral_dark_spot_values_match_the_vendored_preset() {
        let p = ShadcnBase::Neutral.dark();
        assert_eq!(p.background, Color::from_rgb8(0x0A, 0x0A, 0x0A));
        assert_eq!(p.foreground, Color::from_rgb8(0xFA, 0xFA, 0xFA));
        assert_eq!(p.primary, Color::from_rgb8(0xE5, 0xE5, 0xE5));
        // Tailwind's own published sRGB fallback for `red-400`.
        assert_eq!(p.destructive, Color::from_rgb8(0xFF, 0x64, 0x67));
    }

    #[test]
    fn dark_border_and_input_stay_translucent_white() {
        // `oklch(1 0 0 / 10%)` and `/ 15%`: the alpha survives the port, and
        // input reads stronger than border (the source's own ordering).
        let p = ShadcnBase::Neutral.dark();
        let border = p.border.components;
        let input = p.input.components;
        assert_eq!([border[0], border[1], border[2]], [1.0, 1.0, 1.0]);
        assert!(
            (border[3] - 0.1).abs() < 0.01,
            "dark border alpha ≈ 10%, got {}",
            border[3]
        );
        assert!(
            (input[3] - 0.15).abs() < 0.01,
            "dark input alpha ≈ 15%, got {}",
            input[3]
        );
        assert!(input[3] > border[3]);
    }

    #[test]
    fn every_base_ships_both_brightnesses_and_five_chart_slots() {
        for base in ShadcnBase::ALL {
            let (light, dark) = (base.light(), base.dark());
            assert_ne!(
                light.background,
                dark.background,
                "{}: light and dark must differ",
                base.id()
            );
            assert_eq!(light.chart.len(), 5);
            assert_eq!(dark.chart.len(), 5);
            // Every preset's dark sidebar-primary is the shared blue accent.
            assert_eq!(dark.sidebar.primary, Color::from_rgb8(0x14, 0x47, 0xE6));
        }
    }

    #[test]
    fn the_seven_bases_are_distinct_and_ordered_as_the_registry_lists_them() {
        assert_eq!(ShadcnBase::default(), ShadcnBase::Neutral);
        let ids: Vec<&str> = ShadcnBase::ALL.iter().map(|b| b.id()).collect();
        assert_eq!(
            ids,
            [
                "neutral", "stone", "zinc", "mauve", "olive", "mist", "taupe"
            ]
        );
        // Hues differ, so the `foreground` ink differs base to base (neutral is
        // the only achromatic one).
        assert_ne!(
            ShadcnBase::Neutral.light().foreground,
            ShadcnBase::Stone.light().foreground
        );
        assert_ne!(
            ShadcnBase::Zinc.light().foreground,
            ShadcnBase::Olive.light().foreground
        );
    }
}
