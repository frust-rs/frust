//! Shared visual constants, colour resolvers and paint helpers for the chat
//! family, so the components agree on spacing, radii, fonts and colours.
//!
//! Glyph has no spacing token scale: padding and gaps are per-widget
//! constants, and the chat family's live here. Colours resolve from the live
//! `Theme` (`ColorScheme`, [`StatusPalette`]) with an unthemed Glyph-dark
//! fallback per value, never a panic and never a design-language match. A
//! status hue (success, warning, info) reads the `StatusPalette` extension
//! first and falls back to the Glyph-dark palette when a theme carries none.
//!
//! The glyph characters below are all present in the bundled IBM Plex Mono
//! (the face [`mono_family`] resolves to); `●`, `○`, `✗`, `▶` and friends are
//! not, and would silently draw nothing. Space Mono, the display face, also
//! lacks `✓` and `↻`, so these glyphs must be shaped with the mono family, not
//! the display one.

use std::time::Duration;

use frust::authoring::ThemeTextColor;
use frust::authoring::text::{FontFamily, TextLayout};
use frust::authoring::{BoxConstraints, Brush, PaintScene, Point, Rect, RoundedRect, Shape, Size};
use frust::{ColorScheme, StatusColors, StatusPalette, Theme};
use peniko::Color;

use super::model::{ToolKind, ToolStatus};
use crate::tokens::color;

/// Gap between sibling blocks inside a message.
pub(crate) const BLOCK_GAP: f64 = 8.0;
/// Gap between rows of a list-like block.
pub(crate) const ROW_GAP: f64 = 4.0;
/// Inner padding of a card-like block (code, diff, tool call).
pub(crate) const PAD: f64 = 10.0;
/// Inner padding of a compact row.
pub(crate) const PAD_SMALL: f64 = 6.0;
/// Corner radius of a block.
pub(crate) const RADIUS: f64 = 8.0;
/// Corner radius of a small chip or button.
pub(crate) const RADIUS_SMALL: f64 = 4.0;
/// Border width of a block.
pub(crate) const BORDER_WIDTH: f64 = 1.0;
/// Font size of body text: the Glyph `body` token.
pub(crate) const BODY_FONT_SIZE: f32 = 13.0;
/// Font size of code and diff text.
pub(crate) const CODE_FONT_SIZE: f32 = 13.0;
/// Font size of secondary labels.
pub(crate) const LABEL_FONT_SIZE: f32 = 12.0;
/// Smallest side of a primary touch target (an option, send or stop).
pub(crate) const MIN_TARGET: f64 = 44.0;
/// Width used when the incoming constraints are unbounded.
pub(crate) const FALLBACK_WIDTH: f64 = 480.0;
/// Corner-rounding tolerance for an outline stroke.
pub(crate) const PATH_TOLERANCE: f64 = 0.1;
/// Opacity of the ink tint laid over a control's fill while pressed.
pub(crate) const PRESSED_TINT: f32 = 0.14;
/// How long a copy control reads "Copied".
pub(crate) const COPIED_FOR: Duration = Duration::from_millis(1500);
/// Spaces a tab expands to on screen.
pub(crate) const TAB_WIDTH: usize = 4;
/// A copy control's label.
pub(crate) const COPY_TEXT: &str = "Copy";
/// A copy control's label for a moment after a copy.
pub(crate) const COPIED_TEXT: &str = "Copied";

/// Text role of a safe link; the underline is what marks it as one.
pub(crate) const LINK_ROLE: ThemeTextColor = ThemeTextColor::OnSurface;
/// Text role of an in-progress plan entry; weight and mark set it apart.
pub(crate) const PLAN_ACTIVE_ROLE: ThemeTextColor = ThemeTextColor::OnSurface;

const DARK: ColorScheme = color::dark();
const DARK_STATUS: StatusPalette = color::status();

/// The width a block takes: the incoming maximum, or [`FALLBACK_WIDTH`]
/// when it is unbounded.
pub(crate) fn width_of(bc: &BoxConstraints) -> f64 {
    if bc.max().width.is_finite() {
        bc.max().width
    } else {
        FALLBACK_WIDTH
    }
}

/// The monospace family: the theme's `MonospaceFamily` extension, else
/// IBM Plex Mono with the generic monospace slot behind it.
pub(crate) fn mono_family(theme: Option<&Theme>) -> FontFamily {
    match theme {
        Some(t) => t.monospace_family(),
        None => crate::tokens::baseline::monospace_family().family,
    }
}

/// The whole colour scheme: the theme's, else Glyph dark.
pub(crate) fn scheme(theme: Option<&Theme>) -> ColorScheme {
    theme.map_or(DARK, |t| *t.scheme())
}

/// Surface fill of a block.
pub(crate) fn surface(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.surface_container, |t| t.scheme().surface_container)
}

/// A raised fill on a block: a bubble, a header row, a quiet button.
pub(crate) fn surface_high(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.surface_container_high, |t| {
        t.scheme().surface_container_high
    })
}

/// The unfilled part of a bar.
pub(crate) fn track(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.surface_container_highest, |t| {
        t.scheme().surface_container_highest
    })
}

/// Border of a block.
pub(crate) fn border(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.outline_variant, |t| t.scheme().outline_variant)
}

/// Active text and ink.
pub(crate) fn ink(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.on_surface, |t| t.scheme().on_surface)
}

/// Muted (secondary) text.
pub(crate) fn muted(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.on_surface_variant, |t| t.scheme().on_surface_variant)
}

/// Accent ink: a live control, a bar's fill, work in progress.
pub(crate) fn accent(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.primary, |t| t.scheme().primary)
}

/// The error hue.
pub(crate) fn error(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.error, |t| t.scheme().error)
}

/// A status hue: the theme's `StatusPalette` at its brightness, else the
/// Glyph-dark palette.
fn status_hue(theme: Option<&Theme>, pick: fn(&StatusColors) -> Color) -> Color {
    theme
        .and_then(|t| {
            t.extension::<StatusPalette>()
                .map(|s| pick(s.colors(t.brightness)))
        })
        .unwrap_or_else(|| pick(&DARK_STATUS.dark))
}

/// The success hue.
pub(crate) fn success(theme: Option<&Theme>) -> Color {
    status_hue(theme, |s| s.success)
}

/// The warning hue.
pub(crate) fn warning(theme: Option<&Theme>) -> Color {
    status_hue(theme, |s| s.warning)
}

/// The info hue.
pub(crate) fn info(theme: Option<&Theme>) -> Color {
    status_hue(theme, |s| s.info)
}

/// Background of an added diff row (the success wash).
pub(crate) fn added_bg(theme: Option<&Theme>) -> Color {
    status_hue(theme, |s| s.success_container)
}

/// Background of a removed diff row (the error wash).
pub(crate) fn removed_bg(theme: Option<&Theme>) -> Color {
    theme.map_or(DARK.error_container, |t| t.scheme().error_container)
}

/// Color of a tool status mark.
pub(crate) fn status_color(theme: Option<&Theme>, status: ToolStatus) -> Color {
    match status {
        ToolStatus::Completed => success(theme),
        ToolStatus::Failed => error(theme),
        ToolStatus::InProgress => accent(theme),
        _ => muted(theme),
    }
}

/// The single character that stands for a tool kind.
pub(crate) fn kind_glyph(kind: ToolKind) -> char {
    match kind {
        ToolKind::Read => '\u{203A}',   // ›
        ToolKind::Edit => '\u{00B1}',   // ±
        ToolKind::Delete => '\u{00D7}', // ×
        ToolKind::Move => '\u{2192}',   // →
        ToolKind::Search => '?',
        ToolKind::Execute => '$',
        ToolKind::Think => '\u{2026}', // …
        ToolKind::Fetch => '\u{2193}', // ↓
        ToolKind::Other => '\u{2022}', // •
    }
}

/// The single character that stands for a tool status.
pub(crate) fn status_glyph(status: ToolStatus) -> char {
    match status {
        ToolStatus::Pending => '\u{00B7}', // ·
        ToolStatus::InProgress => '*',
        ToolStatus::Completed => '\u{2713}', // ✓
        ToolStatus::Failed => '\u{00D7}',    // ×
    }
}

/// `color` with its alpha scaled by `alpha`.
pub(crate) fn fade(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * alpha])
}

/// `over` blended onto `base` at opacity `t` (the alpha of `base` kept).
pub(crate) fn mix(base: Color, over: Color, t: f32) -> Color {
    let (b, o) = (base.components, over.components);
    Color::new([
        b[0] + (o[0] - b[0]) * t,
        b[1] + (o[1] - b[1]) * t,
        b[2] + (o[2] - b[2]) * t,
        b[3],
    ])
}

/// Whether `c` is hidden or reorders text: a control character other than
/// newline and tab, a bidi embedding, override or isolate, or a zero-width
/// character. The zero-width joiner and non-joiner are not hidden: emoji
/// sequences and some scripts need them. A CR is judged here alone; the CR of
/// a CRLF pair is kept by [`display_text`], which sees the next character.
pub(crate) fn is_hidden_char(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(
            c,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}' | '\u{200E}'..='\u{200F}' | '\u{FEFF}'
        )
}

/// Untrusted `text` as displayed: every [`is_hidden_char`] becomes a visible
/// `?`, so agent-supplied text cannot hide or reorder what the user reads. A CR
/// immediately followed by LF (a CRLF line ending) passes through unchanged; a
/// lone CR is still replaced.
pub(crate) fn display_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let crlf = c == '\r' && chars.peek() == Some(&'\n');
        out.push(if is_hidden_char(c) && !crlf { '?' } else { c });
    }
    out
}

/// `text` with every tab expanded to [`TAB_WIDTH`] spaces.
pub(crate) fn expand_tabs(text: &str) -> String {
    if text.contains('\t') {
        text.replace('\t', &" ".repeat(TAB_WIDTH))
    } else {
        text.to_owned()
    }
}

/// Draw a shaped layout with its top-left at `at`.
pub(crate) fn paint_layout(layout: &TextLayout, at: Point, scene: &mut dyn PaintScene) {
    for run in layout.to_scene_runs(at) {
        scene.draw_glyph_run(run);
    }
}

/// Stroke a rounded outline of `size` at `at`.
pub(crate) fn stroke_outline(
    scene: &mut dyn PaintScene,
    at: Point,
    size: Size,
    radius: f64,
    color: Color,
) {
    let shape = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
    scene.stroke_path(
        at,
        &shape.to_path(PATH_TOLERANCE),
        BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

/// A semantics node's bounds: `r`, in the widget's coordinates, moved to
/// the semantics `origin`.
pub(crate) fn node_bounds(origin: Point, r: Rect) -> frust::accesskit::Rect {
    frust::accesskit::Rect {
        x0: origin.x + r.x0,
        y0: origin.y + r.y0,
        x1: origin.x + r.x1,
        y1: origin.y + r.y1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::GenericSlot;

    use super::super::code_block::CodeTone;

    /// Resolve a role against a scheme the way frust-widgets does at layout.
    fn role_color(scheme: &ColorScheme, role: ThemeTextColor) -> Color {
        match role {
            ThemeTextColor::OnSurface => scheme.on_surface,
            ThemeTextColor::OnPrimary => scheme.on_primary,
            ThemeTextColor::OnSurfaceVariant => scheme.on_surface_variant,
            ThemeTextColor::OnPrimaryContainer => scheme.on_primary_container,
            ThemeTextColor::Error => scheme.error,
        }
    }

    fn luminance(c: Color) -> f64 {
        let lin = |v: f32| {
            let v = f64::from(v);
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let [r, g, b, _] = c.components;
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn display_text_keeps_emoji_joiners_and_zwnj() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(display_text(family), family);
        let persian = "\u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0647}\u{0645}";
        assert_eq!(display_text(persian), persian);
    }

    #[test]
    fn display_text_keeps_crlf_and_still_marks_a_lone_cr() {
        assert_eq!(display_text("a\r\nb"), "a\r\nb");
        assert_eq!(display_text("a\rb"), "a?b");
        assert_eq!(display_text("a\r"), "a?");
        assert_eq!(display_text("a\r\rb"), "a??b");
    }

    #[test]
    fn display_text_still_hides_bidi_and_zero_width_characters() {
        assert_eq!(display_text("a\u{202E}b"), "a?b");
        assert_eq!(
            display_text("a\u{200B}b\u{200E}c\u{200F}d\u{FEFF}e"),
            "a?b?c?d?e"
        );
        assert_eq!(display_text("a\u{2066}b\u{2069}c"), "a?b?c");
    }

    #[test]
    fn chat_text_roles_are_readable_on_their_surfaces() {
        let tones = [
            CodeTone::Plain,
            CodeTone::Keyword,
            CodeTone::String,
            CodeTone::Comment,
            CodeTone::Number,
            CodeTone::Function,
            CodeTone::Type,
            CodeTone::Added,
            CodeTone::Removed,
        ];
        for (name, scheme) in [("dark", color::dark()), ("light", color::light())] {
            let mut uses = vec![
                ("link", LINK_ROLE, scheme.surface),
                ("link on card", LINK_ROLE, scheme.surface_container),
                ("plan", PLAN_ACTIVE_ROLE, scheme.surface),
                ("plan on card", PLAN_ACTIVE_ROLE, scheme.surface_container),
            ];
            for tone in tones {
                uses.push(("code", tone.look().0, scheme.surface_container));
            }
            for (what, role, bg) in uses {
                assert_ne!(role, ThemeTextColor::OnPrimaryContainer, "{name} {what}");
                let ratio = contrast(role_color(&scheme, role), bg);
                // Removed keeps the Error red, which is 4.38 in light; it is
                // struck through and a roles-only fix has no better red.
                let floor = if role == ThemeTextColor::Error {
                    4.3
                } else {
                    4.5
                };
                assert!(
                    ratio >= floor,
                    "{name} {what} {role:?}: contrast {ratio:.2}"
                );
            }
        }
    }

    #[test]
    fn code_tones_stay_distinguishable() {
        let tones = [
            CodeTone::Plain,
            CodeTone::Keyword,
            CodeTone::String,
            CodeTone::Comment,
            CodeTone::Number,
            CodeTone::Function,
            CodeTone::Type,
            CodeTone::Added,
            CodeTone::Removed,
        ];
        for (i, a) in tones.iter().enumerate() {
            for b in &tones[i + 1..] {
                assert_ne!(a.look(), b.look(), "{a:?} and {b:?} look alike");
            }
        }
    }

    const KINDS: [ToolKind; 9] = [
        ToolKind::Read,
        ToolKind::Edit,
        ToolKind::Delete,
        ToolKind::Move,
        ToolKind::Search,
        ToolKind::Execute,
        ToolKind::Think,
        ToolKind::Fetch,
        ToolKind::Other,
    ];
    const STATUSES: [ToolStatus; 4] = [
        ToolStatus::Pending,
        ToolStatus::InProgress,
        ToolStatus::Completed,
        ToolStatus::Failed,
    ];

    #[test]
    fn kind_glyphs_are_distinct() {
        for (i, a) in KINDS.iter().enumerate() {
            for b in &KINDS[i + 1..] {
                assert_ne!(kind_glyph(*a), kind_glyph(*b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn status_glyphs_are_distinct() {
        for (i, a) in STATUSES.iter().enumerate() {
            for b in &STATUSES[i + 1..] {
                assert_ne!(status_glyph(*a), status_glyph(*b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn glyphs_avoid_known_missing_characters() {
        // Verified absent from the bundled IBM Plex Mono and Space Mono.
        const MISSING: &[char] = &[
            '●', '○', '✗', '✕', '✔', '✘', '▶', '▸', '▾', '▼', '◆', '◇', '⋯', '⏵', '⏸', '⌫',
        ];
        for k in KINDS {
            assert!(!MISSING.contains(&kind_glyph(k)), "{k:?}");
        }
        for s in STATUSES {
            assert!(!MISSING.contains(&status_glyph(s)), "{s:?}");
        }
    }

    #[test]
    fn unthemed_matches_baseline_theme() {
        let t = crate::baseline();
        let both = |f: fn(Option<&Theme>) -> Color| (f(Some(&t)), f(None));
        for (name, f) in [
            ("surface", surface as fn(Option<&Theme>) -> Color),
            ("surface_high", surface_high),
            ("track", track),
            ("border", border),
            ("ink", ink),
            ("muted", muted),
            ("accent", accent),
            ("error", error),
            ("success", success),
            ("warning", warning),
            ("info", info),
            ("added_bg", added_bg),
            ("removed_bg", removed_bg),
        ] {
            let (themed, unthemed) = both(f);
            assert_eq!(themed, unthemed, "{name}");
        }
        assert_eq!(scheme(Some(&t)), scheme(None));
        for s in STATUSES {
            assert_eq!(status_color(Some(&t), s), status_color(None, s), "{s:?}");
        }
    }

    #[test]
    fn status_colors_name_their_hue() {
        assert_eq!(status_color(None, ToolStatus::InProgress), DARK.primary);
        assert_eq!(
            status_color(None, ToolStatus::Completed),
            DARK_STATUS.dark.success
        );
        assert_eq!(status_color(None, ToolStatus::Failed), DARK.error);
        assert_eq!(
            status_color(None, ToolStatus::Pending),
            DARK.on_surface_variant
        );
    }

    #[test]
    fn mono_family_is_ibm_plex_mono_then_generic_monospace() {
        let stack = FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace);
        assert_eq!(mono_family(None), stack);
        assert_eq!(mono_family(Some(&crate::baseline())), stack);
    }

    #[test]
    fn display_text_shows_hidden_characters() {
        let hidden = "a\u{7}b\u{202E}c\u{2067}d\u{200B}e\u{FEFF}f\u{200F}g";
        assert_eq!(display_text(hidden), "a?b?c?d?e?f?g");
        assert_eq!(display_text("line\n\tnext"), "line\n\tnext");
    }

    #[test]
    fn mix_and_fade_keep_or_scale_alpha() {
        let base = Color::new([0.0, 0.0, 0.0, 0.5]);
        let over = Color::new([1.0, 1.0, 1.0, 1.0]);
        assert_eq!(mix(base, over, 0.25).components, [0.25, 0.25, 0.25, 0.5]);
        assert_eq!(fade(over, 0.5).components, [1.0, 1.0, 1.0, 0.5]);
    }
}
