//! Brand theme: semantic color tokens anchored on the Frust branding assets,
//! with 256- and 16-color degradation per token, plus the [`Icons`] table
//! (Nerd Font glyphs with a plain-Unicode fallback).
//!
//! # Sampled brand palette (source of truth)
//!
//! TrueColor values are anchored on `docs/assets/branding/`. Hex recorded
//! per token below; the branding PNGs were sampled
//! 2026-07-20 (downscaled + dominant-color histogram):
//!
//! | Token     | Hex       | Source |
//! |-----------|-----------|--------|
//! | `bg`      | `#0D0B09` | near-black background (sampled play-store icon field `#1F1B19` charcoal) |
//! | `surface` | `#1A1612` | charcoal surface (play-store `#1F1B19`) |
//! | `overlay` | `#26201A` | raised overlay / hover wash |
//! | `primary` | `#E1571E` | rust-orange (gear logo sampled `#E5671C`) |
//! | `accent`  | `#F0854F` | soft-orange accent / hovered border |
//! | `fg`      | `#EDE5D8` | cream foreground (badger-stripe off-white) |
//! | `muted`   | `#9B9080` | muted taupe |
//! | `success` | `#10B981` | ready/green |
//! | `warn`    | `#EAB308` | warn/amber |
//! | `error`   | `#F43F5E` | error/red |
//! | `border`  | `#3A322A` | dim charcoal rule (`#3D362E`/`#3A322A`) |
//!
//! The play-store icon's brighter `#CE422B` red-orange and the gear logo's
//! `#E5671C` bracket the chosen `primary` `#E1571E`.

use ratatui::style::Color;

/// The color capability the terminal exposes; each token degrades per depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit direct color (`COLORTERM=truecolor`).
    TrueColor,
    /// 256-color xterm palette.
    Xterm256,
    /// The 16 ANSI system colors.
    Ansi16,
}

impl ColorDepth {
    /// Probe the environment for the richest supported depth. Conservative:
    /// unknown terminals get `Xterm256` (widely safe), never `TrueColor`.
    pub fn detect() -> Self {
        if let Ok(ct) = std::env::var("COLORTERM")
            && (ct.contains("truecolor") || ct.contains("24bit"))
        {
            return ColorDepth::TrueColor;
        }
        match std::env::var("TERM") {
            Ok(term) if term.contains("256") => ColorDepth::Xterm256,
            Ok(term) if term.is_empty() || term == "dumb" => ColorDepth::Ansi16,
            Ok(_) => ColorDepth::Xterm256,
            Err(_) => ColorDepth::Ansi16,
        }
    }
}

/// One semantic color with its per-depth degradation mapping.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColor {
    truecolor: Color,
    x256: Color,
    ansi16: Color,
}

impl ThemeColor {
    const fn new(r: u8, g: u8, b: u8, x256: u8, ansi16: Color) -> Self {
        Self {
            truecolor: Color::Rgb(r, g, b),
            x256: Color::Indexed(x256),
            ansi16,
        }
    }

    /// Resolve to a concrete ratatui color for the given depth.
    pub fn resolve(self, depth: ColorDepth) -> Color {
        match depth {
            ColorDepth::TrueColor => self.truecolor,
            ColorDepth::Xterm256 => self.x256,
            ColorDepth::Ansi16 => self.ansi16,
        }
    }
}

/// Nerd Font icon glyphs with a plain-Unicode fallback, behind one table so a
/// caller never hardcodes a glyph. Defaults to the Unicode set (deterministic
/// in tests and safe on fonts without a Nerd Font patch).
#[derive(Debug, Clone, Copy)]
pub struct Icons {
    /// When `true`, prefer Nerd Font private-use glyphs.
    pub nerd: bool,
}

impl Icons {
    /// Plain-Unicode set (the safe default).
    pub const fn unicode() -> Self {
        Self { nerd: false }
    }

    /// The create/build gear-and-hammer glyph.
    pub fn create(self) -> &'static str {
        if self.nerd {
            "\u{f0ad}" // nf-fa-wrench
        } else {
            "\u{2692}" // ⚒ hammer and pick
        }
    }

    /// The run/play glyph.
    pub fn run(self) -> &'static str {
        if self.nerd {
            "\u{f04b}"
        } else {
            "\u{25b6}" // ▶
        }
    }

    /// The brand gear (titlebar wordmark prefix).
    pub fn gear(self) -> &'static str {
        if self.nerd {
            "\u{f013}"
        } else {
            "\u{2699}" // ⚙
        }
    }

    /// A ready/ok check.
    pub fn ok(self) -> &'static str {
        "\u{2713}" // ✓ (same in both sets)
    }

    /// A hover affordance chevron.
    pub fn chevron(self) -> &'static str {
        "\u{25b8}" // ▸
    }
}

/// The resolved brand theme: semantic tokens + icons + the active color depth.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    depth: ColorDepth,
    t_bg: ThemeColor,
    t_surface: ThemeColor,
    t_overlay: ThemeColor,
    t_primary: ThemeColor,
    t_accent: ThemeColor,
    t_fg: ThemeColor,
    t_muted: ThemeColor,
    t_success: ThemeColor,
    t_warn: ThemeColor,
    t_error: ThemeColor,
    t_border: ThemeColor,
    /// The icon table (Nerd Font vs plain Unicode).
    pub icons: Icons,
}

impl Theme {
    /// The Frust dark brand theme, probing the terminal for its color depth.
    pub fn frust_dark() -> Self {
        Self::frust_dark_at(ColorDepth::detect())
    }

    /// The Frust dark brand theme forced to a specific depth (tests, or a CLI
    /// override later).
    pub fn frust_dark_at(depth: ColorDepth) -> Self {
        Self {
            depth,
            // See the module-level table for the sampled hex per token.
            t_bg: ThemeColor::new(0x0D, 0x0B, 0x09, 232, Color::Black),
            t_surface: ThemeColor::new(0x1A, 0x16, 0x12, 234, Color::Black),
            t_overlay: ThemeColor::new(0x26, 0x20, 0x1A, 236, Color::DarkGray),
            t_primary: ThemeColor::new(0xE1, 0x57, 0x1E, 208, Color::LightRed),
            t_accent: ThemeColor::new(0xF0, 0x85, 0x4F, 215, Color::LightYellow),
            t_fg: ThemeColor::new(0xED, 0xE5, 0xD8, 223, Color::White),
            t_muted: ThemeColor::new(0x9B, 0x90, 0x80, 244, Color::Gray),
            t_success: ThemeColor::new(0x10, 0xB9, 0x81, 42, Color::Green),
            t_warn: ThemeColor::new(0xEA, 0xB3, 0x08, 178, Color::Yellow),
            t_error: ThemeColor::new(0xF4, 0x3F, 0x5E, 197, Color::Red),
            t_border: ThemeColor::new(0x3A, 0x32, 0x2A, 238, Color::DarkGray),
            icons: Icons::unicode(),
        }
    }

    /// The active color depth.
    pub fn depth(&self) -> ColorDepth {
        self.depth
    }

    /// Near-black application background.
    pub fn bg(&self) -> Color {
        self.t_bg.resolve(self.depth)
    }
    /// Charcoal surface fill (panels, sidebar).
    pub fn surface(&self) -> Color {
        self.t_surface.resolve(self.depth)
    }
    /// Raised overlay / hover wash.
    pub fn overlay(&self) -> Color {
        self.t_overlay.resolve(self.depth)
    }
    /// Rust-orange primary.
    pub fn primary(&self) -> Color {
        self.t_primary.resolve(self.depth)
    }
    /// Soft-orange accent (hovered borders).
    pub fn accent(&self) -> Color {
        self.t_accent.resolve(self.depth)
    }
    /// Cream foreground.
    pub fn fg(&self) -> Color {
        self.t_fg.resolve(self.depth)
    }
    /// Muted taupe secondary text.
    pub fn muted(&self) -> Color {
        self.t_muted.resolve(self.depth)
    }
    /// Success green.
    pub fn success(&self) -> Color {
        self.t_success.resolve(self.depth)
    }
    /// Warning amber.
    pub fn warn(&self) -> Color {
        self.t_warn.resolve(self.depth)
    }
    /// Error red.
    pub fn error(&self) -> Color {
        self.t_error.resolve(self.depth)
    }
    /// Dim charcoal border/rule.
    pub fn border(&self) -> Color {
        self.t_border.resolve(self.depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degradation_maps_per_depth() {
        let tc = ThemeColor::new(0xE1, 0x57, 0x1E, 208, Color::LightRed);
        assert_eq!(
            tc.resolve(ColorDepth::TrueColor),
            Color::Rgb(0xE1, 0x57, 0x1E)
        );
        assert_eq!(tc.resolve(ColorDepth::Xterm256), Color::Indexed(208));
        assert_eq!(tc.resolve(ColorDepth::Ansi16), Color::LightRed);
    }

    #[test]
    fn theme_resolves_primary_at_depth() {
        let t = Theme::frust_dark_at(ColorDepth::TrueColor);
        assert_eq!(t.primary(), Color::Rgb(0xE1, 0x57, 0x1E));
        let t16 = Theme::frust_dark_at(ColorDepth::Ansi16);
        assert_eq!(t16.primary(), Color::LightRed);
    }

    #[test]
    fn icons_have_unicode_fallback() {
        let i = Icons::unicode();
        assert_eq!(i.create(), "\u{2692}");
        let n = Icons { nerd: true };
        assert_ne!(n.create(), i.create());
    }
}
