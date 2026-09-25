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
    /// `true` selects the macOS `⌘`/`⌥` key-glyph spelling
    /// ([`crate::engine::palette::key_glyphs_for`]); `false` selects the
    /// spelled-out `^X`/`Alt+x` notation used everywhere else. Threaded
    /// through render calls as a plain value rather than a process-global —
    /// see [`Self::palette_open_hint`]/[`Self::mouse_toggle_hint`].
    macos_glyphs: bool,
}

impl Theme {
    /// Xterm256 shimmer ramp (`ui::anim::shimmer`'s `themed_shimmer_spans`):
    /// a short, fixed set of palette indices approximating the TrueColor
    /// muted→accent sweep through the 256-cube's own quantization.
    /// `ThemeColor::resolve` only ever answers one of two fixed
    /// `Color::Indexed` endpoints at this depth — there is no live RGB blend
    /// to bucket a per-character `t` into — so the ramp is a small
    /// hand-picked table rather than a `lerp_color` computation. Endpoints
    /// are pinned to this theme's actual `t_muted`/`t_accent` x256 indices
    /// (244/215 below); the three interior steps are cube entries whose own
    /// RGB roughly tracks a straight-line lerp between muted's resolved gray
    /// `(128,128,128)` and accent's resolved orange `(255,175,95)` (worked
    /// out once, offline, against the standard 6×6×6 cube + grayscale-ramp
    /// layout) — a visual judgment call within that constraint, not a
    /// nearest-color search:
    ///
    /// | Step | Index | Cube/grayscale RGB   |
    /// |------|-------|-----------------------|
    /// | 0    | 244   | (128,128,128) — muted |
    /// | 1    | 246   | (148,148,148)         |
    /// | 2    | 137   | (175,135,95)          |
    /// | 3    | 173   | (215,135,95)          |
    /// | 4    | 215   | (255,175,95) — accent |
    ///
    /// Keep in lockstep with `t_muted`/`t_accent`'s x256 fields below if the
    /// brand palette is ever re-sampled — the two endpoints must always
    /// match.
    pub const SHIMMER_RAMP_X256: [u8; 5] = [244, 246, 137, 173, 215];

    /// The Frust dark brand theme, probing the terminal for its color depth.
    pub fn frust_dark() -> Self {
        Self::frust_dark_at(ColorDepth::detect())
    }

    /// The Frust dark brand theme forced to a specific depth (tests, or a CLI
    /// override later). Key glyphs are the real build target's
    /// (`cfg!(target_os = "macos")`) — the production path.
    pub fn frust_dark_at(depth: ColorDepth) -> Self {
        Self::frust_dark_with_glyphs(depth, cfg!(target_os = "macos"))
    }

    /// [`Self::frust_dark_at`] with the macOS key-glyph spelling forced
    /// regardless of the actual build target. Used only by the insta
    /// snapshot harness (`crates/frust-tui/tests/snapshots.rs`'s render
    /// helper) so every CI host — Linux, macOS, Windows — renders the
    /// identical glyphs and no `.snap` fixture needs a per-host fork;
    /// production code never calls this.
    pub fn frust_dark_at_macos(depth: ColorDepth) -> Self {
        Self::frust_dark_with_glyphs(depth, true)
    }

    fn frust_dark_with_glyphs(depth: ColorDepth, macos_glyphs: bool) -> Self {
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
            macos_glyphs,
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

    /// `true` when this theme carries the macOS key-glyph spelling.
    pub fn macos_glyphs(&self) -> bool {
        self.macos_glyphs
    }

    /// The command-palette-open hint for this theme's host glyph set (see
    /// [`crate::engine::palette::key_glyphs_for`]). Shared by the workbench
    /// and welcome status bars.
    pub fn palette_open_hint(&self) -> &'static str {
        crate::engine::palette::key_glyphs_for(self.macos_glyphs).0
    }

    /// The mouse-capture-toggle key hint for this theme's host glyph set
    /// (see [`crate::engine::palette::key_glyphs_for`]). Shared by the
    /// status-bar mouse chip and the help overlay's "Toggle mouse capture"
    /// row.
    pub fn mouse_toggle_hint(&self) -> &'static str {
        crate::engine::palette::key_glyphs_for(self.macos_glyphs).1
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

    /// A theme built with the macOS glyph set forced carries the `⌘`/`⌥`
    /// spelling through both hint accessors, regardless of the actual build
    /// target — the path `crates/frust-tui/tests/snapshots.rs` relies on for
    /// host-independent fixtures.
    #[test]
    fn macos_glyph_theme_carries_the_mac_hints() {
        let theme = Theme::frust_dark_at_macos(ColorDepth::TrueColor);
        assert!(theme.macos_glyphs());
        assert_eq!(theme.palette_open_hint(), "⌘ palette");
        assert_eq!(theme.mouse_toggle_hint(), "⌥m");
    }

    /// [`Theme::frust_dark_with_glyphs`] with the non-macOS set carries the
    /// spelled-out `^P`/`Alt+m` hints through both accessors.
    #[test]
    fn non_macos_glyph_theme_carries_the_spelled_out_hints() {
        let theme = Theme::frust_dark_with_glyphs(ColorDepth::TrueColor, false);
        assert!(!theme.macos_glyphs());
        assert_eq!(theme.palette_open_hint(), "^P palette");
        assert_eq!(theme.mouse_toggle_hint(), "Alt+m");
    }
}
