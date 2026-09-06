//! Pure color-math utilities for a left-to-right shimmer sweep effect.
//!
//! A "shimmer" is a bright "head" that sweeps across text from left to
//! right, brightening each character's foreground based on its distance from
//! the head. The exact mechanism depends on the terminal's color depth
//! (`ColorDepth`, resolved once per [`crate::ui::theme::Theme`]) since the
//! three depths do not offer the same palette:
//!
//! - **TrueColor**: [`shimmer_spans`] lerps each character's fg between a
//!   `base` dim color and a `highlight` bright color via [`lerp_color`] — a
//!   real per-frame RGB blend.
//! - **Xterm256**: [`shimmer_spans_indexed`] buckets the same per-character
//!   `t` into a short fixed ramp of palette indices ([`ramp_color`],
//!   `Theme::SHIMMER_RAMP_X256`) approximating the TrueColor sweep through
//!   the 256-cube's own quantization — a real, visible sweep, not a
//!   constant color.
//! - **Ansi16**: [`shimmer_spans_flat_bold`] has no palette to sweep
//!   through between two brand tokens, so it degrades to a flat `base`
//!   foreground with `Modifier::BOLD` emphasis on characters near the head —
//!   motion without color change. See `docs/LIMITATIONS.md`'s
//!   `tui-shimmer-ansi16-degrade`.
//!
//! [`shimmer_spans`]/[`shimmer_spans_indexed`]/[`shimmer_spans_flat_bold`]
//! are intentionally **pure** color math with no `Theme` dependency (no
//! `AppState`, no rendering side effects, no I/O), so each is trivially
//! testable and reusable with any color/ramp. [`themed_shimmer_spans`] is
//! the call-site convenience that picks the right one of the three per
//! `theme.depth()` and resolves colors from the TUI's own
//! [`crate::ui::theme::Theme`] — never a hardcoded RGB palette.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::ui::theme::{ColorDepth, Theme};

/// Frames per full shimmer sweep (~1.5 s at the 50 ms / 20 fps tick cadence
/// — see `crate::runner::TICK`).
const SHIMMER_PERIOD_FRAMES: u64 = 30;

/// Width of the bright "head" of the sweep, in characters.
const SHIMMER_HEAD_WIDTH: f32 = 3.5;

/// How far (in characters) the sweep head travels off-screen past each edge,
/// so it fades in from the left, exits off the right, and leaves a brief
/// all-dim rest gap between cycles instead of popping in / snapping back.
const SHIMMER_LEAD: f32 = 3.0;

/// Linearly interpolate between two `Color::Rgb` colors. `t` is clamped to
/// `[0.0, 1.0]`.
///
/// If either color is not `Color::Rgb`, returns `a` unchanged. In practice
/// this only ever runs at `ColorDepth::TrueColor` — [`themed_shimmer_spans`]
/// routes `Xterm256`/`Ansi16` through [`shimmer_spans_indexed`]/
/// [`shimmer_spans_flat_bold`] instead, neither of which calls this
/// function, so the fallback arm is a defensive default for a direct caller
/// passing a non-`Rgb` pair, not a real code path.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
        }
        _ => a,
    }
}

/// Bucket `t` (`[0.0, 1.0]`, clamped) into one of `ramp`'s indices — the
/// discrete-palette analogue of [`lerp_color`] for a depth with no live RGB
/// blend (`Xterm256`, see `Theme::SHIMMER_RAMP_X256`). `ramp` must be
/// non-empty; the sole caller ([`shimmer_spans_indexed`]) always passes a
/// fixed non-empty table, so an empty slice only reaches here through a
/// direct misuse — `Color::Reset` is a defensive, visually-inert fallback
/// for that case rather than a panic.
pub fn ramp_color(ramp: &[u8], t: f32) -> Color {
    let Some(last) = ramp.len().checked_sub(1) else {
        return Color::Reset;
    };
    let t = t.clamp(0.0, 1.0);
    let idx = (t * last as f32).round() as usize;
    Color::Indexed(ramp[idx.min(last)])
}

/// Head position (in character units, may be negative or `>= n`) for a sweep
/// over `n` characters at `phase`. Shared by [`shimmer_spans`],
/// [`shimmer_spans_indexed`], and [`shimmer_spans_flat_bold`] so the three
/// depth strategies sweep in lockstep.
fn shimmer_head(n: f32, phase: f32) -> f32 {
    phase * (n + SHIMMER_LEAD * 2.0) - SHIMMER_LEAD
}

/// Per-character brightness `t` (`1.0` at the head, `0.0` at/beyond
/// `SHIMMER_HEAD_WIDTH` characters away) for character index `i` given the
/// sweep `head` position. Shared by the three depth strategies, see
/// [`shimmer_head`].
fn shimmer_char_t(i: f32, head: f32) -> f32 {
    let dist = (i - head).abs();
    (1.0 - dist / SHIMMER_HEAD_WIDTH).max(0.0)
}

/// Current sweep position in `[0.0, 1.0)`, derived from the global animation
/// frame (`AppState::animation_frame`). Wraps cleanly via modulo so `u64`
/// wrap in the source frame is fine.
pub fn shimmer_phase(frame: u64) -> f32 {
    (frame % SHIMMER_PERIOD_FRAMES) as f32 / SHIMMER_PERIOD_FRAMES as f32
}

/// Build shimmered spans for `text`: each character's fg is lerped between
/// `base` and `highlight` based on its distance from a head that sweeps left
/// to right as `phase` advances. `modifier` (e.g. `BOLD`) is applied to every
/// span so the caller's emphasis is preserved. Empty `text` yields no spans.
pub fn shimmer_spans(
    text: &str,
    base: Color,
    highlight: Color,
    phase: f32,
    modifier: Modifier,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    // Map the head position into [-SHIMMER_LEAD, n + SHIMMER_LEAD) so it
    // travels off-screen on both sides. This produces a soft fade-in from
    // the left edge, a soft fade-out off the right edge, and a brief
    // all-dim rest gap between cycles — instead of snapping back to
    // character 0.
    let n = chars.len() as f32;
    let head = shimmer_head(n, phase);
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let t = shimmer_char_t(i as f32, head); // 1 at head → 0 away
            let fg = lerp_color(base, highlight, t);
            Span::styled(
                c.to_string(),
                Style::default().fg(fg).add_modifier(modifier),
            )
        })
        .collect()
}

/// [`shimmer_spans`]'s `Xterm256` counterpart: instead of a live RGB lerp,
/// buckets each character's `t` into `ramp` via [`ramp_color`]. Same head
/// motion/timing as `shimmer_spans` (`shimmer_head`/`shimmer_char_t`), so
/// the sweep tracks identically across depths — only the color source
/// differs. Empty `text` yields no spans, matching `shimmer_spans`.
pub fn shimmer_spans_indexed(
    text: &str,
    ramp: &[u8],
    phase: f32,
    modifier: Modifier,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let n = chars.len() as f32;
    let head = shimmer_head(n, phase);
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let t = shimmer_char_t(i as f32, head);
            let fg = ramp_color(ramp, t);
            Span::styled(
                c.to_string(),
                Style::default().fg(fg).add_modifier(modifier),
            )
        })
        .collect()
}

/// The width of `shimmer_char_t`'s bright zone (`t >= this`) that earns
/// `Modifier::BOLD` in [`shimmer_spans_flat_bold`]. Chosen so roughly the
/// same head-width characters that would visibly brighten under
/// [`shimmer_spans`]/[`shimmer_spans_indexed`] go bold here too, keeping the
/// three depths' sweeps the same apparent width.
const SHIMMER_ANSI16_BOLD_THRESHOLD: f32 = 0.5;

/// `Ansi16`'s degrade for [`shimmer_spans`]: there is no intermediate
/// palette between two brand tokens at this depth (`docs/LIMITATIONS.md`'s
/// `tui-shimmer-ansi16-degrade`), so every character stays a flat `base`
/// foreground and the sweep is expressed purely as `Modifier::BOLD` on
/// characters within [`SHIMMER_ANSI16_BOLD_THRESHOLD`] of the head. Same
/// head motion/timing as `shimmer_spans`. Empty `text` yields no spans.
pub fn shimmer_spans_flat_bold(
    text: &str,
    base: Color,
    phase: f32,
    modifier: Modifier,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let n = chars.len() as f32;
    let head = shimmer_head(n, phase);
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let t = shimmer_char_t(i as f32, head);
            let mut m = modifier;
            if t >= SHIMMER_ANSI16_BOLD_THRESHOLD {
                m |= Modifier::BOLD;
            }
            Span::styled(c.to_string(), Style::default().fg(base).add_modifier(m))
        })
        .collect()
}

/// The shimmer sweep resolved against the TUI's own brand [`Theme`], picking
/// the strategy that matches `theme.depth()` (see the module doc): a
/// TrueColor RGB lerp ([`shimmer_spans`], `base` = `theme.muted()`,
/// `highlight` = `theme.accent()`), an Xterm256 ramp bucket
/// ([`shimmer_spans_indexed`], `Theme::SHIMMER_RAMP_X256`), or an Ansi16
/// flat+BOLD degrade ([`shimmer_spans_flat_bold`], `base` = `theme.muted()`).
/// `phase` is derived from `frame` via [`shimmer_phase`] in every case —
/// the shape a status-line caller wants
/// (`themed_shimmer_spans(label, state.animation_frame, theme,
/// Modifier::BOLD)`), so no call site hardcodes a color pair, a ramp, or a
/// depth check itself.
pub fn themed_shimmer_spans(
    text: &str,
    frame: u64,
    theme: &Theme,
    modifier: Modifier,
) -> Vec<Span<'static>> {
    let phase = shimmer_phase(frame);
    match theme.depth() {
        ColorDepth::TrueColor => {
            shimmer_spans(text, theme.muted(), theme.accent(), phase, modifier)
        }
        ColorDepth::Xterm256 => {
            shimmer_spans_indexed(text, &Theme::SHIMMER_RAMP_X256, phase, modifier)
        }
        ColorDepth::Ansi16 => shimmer_spans_flat_bold(text, theme.muted(), phase, modifier),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(200, 100, 50);

        // t=0.0 → a
        assert_eq!(lerp_color(a, b, 0.0), a);

        // t=1.0 → b
        assert_eq!(lerp_color(a, b, 1.0), b);

        // t=0.5 → component-wise midpoint (rounded)
        let mid = lerp_color(a, b, 0.5);
        assert_eq!(mid, Color::Rgb(100, 50, 25));
    }

    #[test]
    fn lerp_non_rgb_falls_back_to_base() {
        let a = Color::Yellow;
        let b = Color::Rgb(88, 166, 255);

        // When `a` is non-RGB, returns `a` unchanged regardless of t.
        assert_eq!(lerp_color(a, b, 0.5), a);
        assert_eq!(lerp_color(a, b, 1.0), a);

        // When `b` is non-RGB but `a` is RGB, also falls back to `a`.
        let a2 = Color::Rgb(88, 166, 255);
        let b2 = Color::Green;
        assert_eq!(lerp_color(a2, b2, 0.5), a2);
    }

    #[test]
    fn lerp_clamps_t_outside_range() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 100, 100);

        // t < 0.0 clamped to 0.0 → a
        assert_eq!(lerp_color(a, b, -1.0), a);

        // t > 1.0 clamped to 1.0 → b
        assert_eq!(lerp_color(a, b, 2.0), b);
    }

    #[test]
    fn shimmer_phase_wraps_over_period() {
        // phase(0) and phase(SHIMMER_PERIOD_FRAMES) must be identical (both 0.0)
        let phase_at_zero = shimmer_phase(0);
        let phase_at_period = shimmer_phase(SHIMMER_PERIOD_FRAMES);
        assert_eq!(phase_at_zero, phase_at_period);
        assert_eq!(phase_at_zero, 0.0);

        // Phase must be in [0.0, 1.0)
        for frame in 0..SHIMMER_PERIOD_FRAMES {
            let p = shimmer_phase(frame);
            assert!(
                (0.0..1.0).contains(&p),
                "phase {p} out of [0, 1) for frame {frame}"
            );
        }
    }

    #[test]
    fn shimmer_phase_no_panic_near_u64_max() {
        // Must not panic near u64::MAX (wraps via %)
        let _ = shimmer_phase(u64::MAX);
        let _ = shimmer_phase(u64::MAX - 1);
    }

    #[test]
    fn shimmer_spans_one_per_char_and_empty() {
        // Empty text → empty Vec
        let spans = shimmer_spans(
            "",
            Color::Rgb(0, 0, 0),
            Color::Rgb(255, 255, 255),
            0.0,
            Modifier::empty(),
        );
        assert!(spans.is_empty());

        // Non-empty text → one span per character
        let text = "Hello";
        let spans = shimmer_spans(
            text,
            Color::Rgb(0, 0, 0),
            Color::Rgb(255, 255, 255),
            0.0,
            Modifier::empty(),
        );
        assert_eq!(spans.len(), text.chars().count());

        // Verify each span's content is a single character matching the input
        for (span, ch) in spans.iter().zip(text.chars()) {
            assert_eq!(span.content.as_ref(), ch.to_string().as_str());
        }
    }

    #[test]
    fn shimmer_spans_head_is_brightest() {
        let base = Color::Rgb(50, 50, 50);
        let highlight = Color::Rgb(250, 250, 250);
        let text = "ABCDEFGHIJ"; // 10 chars, n=10

        // Choose a phase that places the head over interior index 4.
        // head = phase * (n + SHIMMER_LEAD*2) - SHIMMER_LEAD
        //      = phase * 16 - 3 = 4  →  phase = 7/16 = 0.4375
        let phase = 7.0_f32 / 16.0;
        let spans = shimmer_spans(text, base, highlight, phase, Modifier::empty());

        // Character 4 is nearest the head and should be brightest.
        let fg_at_head = match spans[4].style.fg {
            Some(Color::Rgb(r, _, _)) => r,
            _ => panic!("expected Rgb color at index 4"),
        };

        // Character 0: dist = |0 - 4| = 4 > SHIMMER_HEAD_WIDTH(3.5) → base
        let fg_far_left = spans[0].style.fg;
        assert_eq!(
            fg_far_left,
            Some(base),
            "char at dist >3.5 from head (index 0) should equal base"
        );

        // Character 9: dist = |9 - 4| = 5 > SHIMMER_HEAD_WIDTH(3.5) → base
        let fg_far_right = spans[9].style.fg;
        assert_eq!(
            fg_far_right,
            Some(base),
            "char at dist >3.5 from head (index 9) should equal base"
        );

        // Index 4 must be brighter than both far ends.
        let base_r = match base {
            Color::Rgb(r, _, _) => r,
            _ => panic!("expected Rgb"),
        };
        assert!(
            fg_at_head > base_r,
            "head char r={fg_at_head} should be brighter than base r={base_r}"
        );
    }

    /// At `phase = 0.0` the head is at `-SHIMMER_LEAD = -3.0`. Index 0 is
    /// `3.0` characters away → `t ≈ 0.143` (dim, not full highlight). This
    /// proves there is no pop-in at cycle start.
    #[test]
    fn shimmer_spans_no_pop_in_at_phase_zero() {
        let base = Color::Rgb(50, 50, 50);
        let highlight = Color::Rgb(250, 250, 250);
        let text = "ABCDEFGHIJ"; // 10 chars

        let spans = shimmer_spans(text, base, highlight, 0.0, Modifier::empty());

        // At phase=0.0, head = -3.0. Index 0 is only 3/3.5 of the way from
        // the head to off-screen → dim, not full highlight.
        let fg_0 = match spans[0].style.fg {
            Some(Color::Rgb(r, _, _)) => r,
            _ => panic!("expected Rgb at index 0"),
        };
        let base_r = match base {
            Color::Rgb(r, _, _) => r,
            _ => panic!("expected Rgb base"),
        };
        let highlight_r = match highlight {
            Color::Rgb(r, _, _) => r,
            _ => panic!("expected Rgb highlight"),
        };
        let midpoint_r = (base_r as u16 + highlight_r as u16) / 2;

        // Index 0 should be closer to base than to highlight (no pop-in).
        assert!(
            fg_0 < midpoint_r as u8,
            "at phase=0.0, index 0 r={fg_0} should be closer to base ({base_r}) than highlight ({highlight_r})"
        );
    }

    /// There exists a phase range where the head is fully off-screen to the
    /// right and every character is at `base` (the rest gap).
    ///
    /// For n=10, SHIMMER_LEAD=3.0, SHIMMER_HEAD_WIDTH=3.5:
    ///   head > (n-1) + SHIMMER_HEAD_WIDTH  →  head > 12.5
    ///   phase * 16 - 3 > 12.5  →  phase > 15.5/16 ≈ 0.96875
    ///
    /// At phase=0.97 the head is at 12.52, 3.52 past the last char → t=0 for
    /// all.
    #[test]
    fn shimmer_spans_rest_gap_all_at_base() {
        let base = Color::Rgb(50, 50, 50);
        let highlight = Color::Rgb(250, 250, 250);
        let text = "ABCDEFGHIJ"; // 10 chars

        let spans = shimmer_spans(text, base, highlight, 0.97, Modifier::empty());

        // Every character must equal base (head is fully off-screen right).
        for (i, span) in spans.iter().enumerate() {
            assert_eq!(
                span.style.fg,
                Some(base),
                "at phase=0.97, index {i} should be at base (rest gap)"
            );
        }
    }

    #[test]
    fn shimmer_spans_preserves_modifier() {
        let spans = shimmer_spans(
            "Bold",
            Color::Rgb(50, 50, 50),
            Color::Rgb(200, 200, 200),
            0.5,
            Modifier::BOLD,
        );
        for span in &spans {
            assert!(
                span.style.add_modifier.contains(Modifier::BOLD),
                "every span must carry BOLD modifier"
            );
        }
    }

    #[test]
    fn shimmer_spans_unicode_multibyte() {
        // Unicode text: each char produces one span, not one byte
        let text = "→✓★";
        let spans = shimmer_spans(
            text,
            Color::Rgb(0, 0, 0),
            Color::Rgb(255, 255, 255),
            0.0,
            Modifier::empty(),
        );
        assert_eq!(spans.len(), 3, "3 Unicode chars → 3 spans");
    }

    #[test]
    fn themed_shimmer_uses_theme_muted_and_accent() {
        let theme = Theme::frust_dark_at(ColorDepth::TrueColor);
        let spans = themed_shimmer_spans("hi", 0, &theme, Modifier::empty());
        assert_eq!(spans.len(), 2);
        // Every fg must be a lerp of theme.muted()/theme.accent(), i.e. an
        // Rgb color at TrueColor depth — never a fdemon-hardcoded literal.
        for span in &spans {
            assert!(matches!(span.style.fg, Some(Color::Rgb(_, _, _))));
        }
    }

    #[test]
    fn themed_shimmer_phase_matches_shimmer_phase() {
        let theme = Theme::frust_dark_at(ColorDepth::TrueColor);
        let frame = 15;
        let direct = shimmer_spans(
            "hi",
            theme.muted(),
            theme.accent(),
            shimmer_phase(frame),
            Modifier::empty(),
        );
        let themed = themed_shimmer_spans("hi", frame, &theme, Modifier::empty());
        assert_eq!(direct, themed);
    }

    // ── Xterm256: indexed ramp sweep (fixes review Major M4) ────────────────

    #[test]
    fn ramp_color_endpoints_and_midpoint() {
        let ramp = [10u8, 20, 30, 40, 50];

        assert_eq!(ramp_color(&ramp, 0.0), Color::Indexed(10));
        assert_eq!(ramp_color(&ramp, 1.0), Color::Indexed(50));
        // t=0.5 → the middle bucket of a 5-entry ramp.
        assert_eq!(ramp_color(&ramp, 0.5), Color::Indexed(30));
    }

    #[test]
    fn ramp_color_clamps_t_outside_range() {
        let ramp = [10u8, 20, 30];
        assert_eq!(ramp_color(&ramp, -1.0), Color::Indexed(10));
        assert_eq!(ramp_color(&ramp, 2.0), Color::Indexed(30));
    }

    #[test]
    fn ramp_color_empty_ramp_is_defensive_reset() {
        // Not a real call path (themed_shimmer_spans always passes a fixed
        // non-empty table) — just proves no panic on misuse.
        assert_eq!(ramp_color(&[], 0.5), Color::Reset);
    }

    /// Mirrors `shimmer_spans_head_is_brightest`: an Indexed-pair sweep
    /// actually moves through the ramp's steps as the head passes — the bug
    /// this task fixes made this a constant `base` for every `t`.
    #[test]
    fn shimmer_spans_indexed_moves_through_ramp_steps() {
        let ramp = [244u8, 246, 137, 173, 215];
        let text = "ABCDEFGHIJ"; // 10 chars, n=10

        // Same phase as shimmer_spans_head_is_brightest: head lands on index 4.
        let phase = 7.0_f32 / 16.0;
        let spans = shimmer_spans_indexed(text, &ramp, phase, Modifier::empty());

        // Head (index 4) must resolve to the ramp's brightest (last) entry.
        assert_eq!(spans[4].style.fg, Some(Color::Indexed(215)));
        // Far ends (dist > SHIMMER_HEAD_WIDTH) must resolve to the ramp's
        // dimmest (first) entry — the muted endpoint, not a constant
        // mid-ramp value.
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(244)));
        assert_eq!(spans[9].style.fg, Some(Color::Indexed(244)));

        // At least one interior character must land on an intermediate
        // ramp step — this is the actual "sweep" assertion: the rendered
        // colors are not just the two endpoints.
        let distinct: std::collections::BTreeSet<u8> = spans
            .iter()
            .filter_map(|s| match s.style.fg {
                Some(Color::Indexed(idx)) => Some(idx),
                _ => None,
            })
            .collect();
        assert!(
            distinct.len() > 2,
            "expected more than 2 distinct indexed colors across the sweep, got {distinct:?}"
        );
    }

    #[test]
    fn shimmer_spans_indexed_empty_text() {
        let ramp = [244u8, 215];
        assert!(shimmer_spans_indexed("", &ramp, 0.0, Modifier::empty()).is_empty());
    }

    #[test]
    fn shimmer_spans_indexed_preserves_modifier() {
        let ramp = [244u8, 215];
        let spans = shimmer_spans_indexed("Bold", &ramp, 0.5, Modifier::BOLD);
        for span in &spans {
            assert!(span.style.add_modifier.contains(Modifier::BOLD));
        }
    }

    // ── Ansi16: flat base + BOLD-head degrade ────────────────────────────────

    #[test]
    fn shimmer_spans_flat_bold_is_flat_color_everywhere() {
        let base = Color::Gray;
        let text = "ABCDEFGHIJ"; // 10 chars
        let phase = 7.0_f32 / 16.0; // head over index 4, matching the TrueColor test

        let spans = shimmer_spans_flat_bold(text, base, phase, Modifier::empty());

        // Every span's fg is the flat base color, at the head and away from it.
        for (i, span) in spans.iter().enumerate() {
            assert_eq!(
                span.style.fg,
                Some(base),
                "index {i} fg should stay flat base, never lerp toward an accent"
            );
        }
    }

    #[test]
    fn shimmer_spans_flat_bold_bolds_only_near_head() {
        let base = Color::Gray;
        let text = "ABCDEFGHIJ"; // 10 chars
        let phase = 7.0_f32 / 16.0; // head over index 4

        let spans = shimmer_spans_flat_bold(text, base, phase, Modifier::empty());

        // The head itself is bold (t = 1.0 >= threshold).
        assert!(
            spans[4].style.add_modifier.contains(Modifier::BOLD),
            "head character should carry BOLD"
        );
        // Far ends (t = 0.0) are not bold.
        assert!(
            !spans[0].style.add_modifier.contains(Modifier::BOLD),
            "far-left character should not carry BOLD"
        );
        assert!(
            !spans[9].style.add_modifier.contains(Modifier::BOLD),
            "far-right character should not carry BOLD"
        );
    }

    #[test]
    fn shimmer_spans_flat_bold_preserves_caller_modifier() {
        // A caller-supplied modifier (e.g. an italic flag) must survive
        // alongside the sweep's own conditional BOLD, everywhere.
        let base = Color::Gray;
        let spans = shimmer_spans_flat_bold("Hi", base, 0.0, Modifier::ITALIC);
        for span in &spans {
            assert!(span.style.add_modifier.contains(Modifier::ITALIC));
        }
    }

    #[test]
    fn shimmer_spans_flat_bold_empty_text() {
        assert!(shimmer_spans_flat_bold("", Color::Gray, 0.0, Modifier::empty()).is_empty());
    }

    // ── themed_shimmer_spans: per-depth dispatch ─────────────────────────────

    #[test]
    fn themed_shimmer_xterm256_sweeps_through_ramp() {
        let theme = Theme::frust_dark_at(ColorDepth::Xterm256);
        let phase = 7.0_f32 / 16.0;
        let frame = (phase * SHIMMER_PERIOD_FRAMES as f32).round() as u64;
        let text = "ABCDEFGHIJ";

        let spans = themed_shimmer_spans(text, frame, &theme, Modifier::empty());

        // Every fg must be Indexed (never a plain named/Rgb color) and must
        // vary across the sweep — not the constant muted base the pre-fix
        // code produced for every t at this depth.
        for span in &spans {
            assert!(matches!(span.style.fg, Some(Color::Indexed(_))));
        }
        let distinct: std::collections::BTreeSet<u8> = spans
            .iter()
            .filter_map(|s| match s.style.fg {
                Some(Color::Indexed(idx)) => Some(idx),
                _ => None,
            })
            .collect();
        assert!(
            distinct.len() > 1,
            "Xterm256 shimmer must not collapse to a single constant color, got {distinct:?}"
        );
    }

    #[test]
    fn themed_shimmer_ansi16_is_flat_plus_bold() {
        let theme = Theme::frust_dark_at(ColorDepth::Ansi16);
        let phase = 7.0_f32 / 16.0;
        let frame = (phase * SHIMMER_PERIOD_FRAMES as f32).round() as u64;
        let text = "ABCDEFGHIJ";

        let spans = themed_shimmer_spans(text, frame, &theme, Modifier::empty());

        // Every fg is the theme's flat muted() Ansi16 color — no sweep color
        // change is possible at this depth.
        let muted = theme.muted();
        for span in &spans {
            assert_eq!(span.style.fg, Some(muted));
        }
        // But the head is expressed as BOLD, so the sweep is still visible.
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "expected at least one bold (head) character in the Ansi16 sweep"
        );
        assert!(
            spans
                .iter()
                .any(|s| !s.style.add_modifier.contains(Modifier::BOLD)),
            "expected at least one non-bold (away-from-head) character too"
        );
    }
}
