//! [`TextContext`]: the heavyweight, `!Sync` owner of parley's font and layout
//! state.
//!
//! Created once and threaded through the app/render loop (spec §10.3). It holds
//! a [`parley::FontContext`] (system font sources, loaded via fontique — Core
//! Text on macOS with no registration) and a [`parley::LayoutContext`] scratch
//! buffer reused across layout passes.

use parley::style::{GenericFamily, StyleProperty};
use peniko::Brush;

use crate::layout::TextLayout;
use crate::style::TextStyle;

/// Owns parley's font matching and layout scratch state.
///
/// This is deliberately not `Clone`/`Sync`: it is expensive per-instance state
/// meant to be constructed once and borrowed mutably for each layout pass. The
/// generic brush parameter is fixed to [`peniko::Brush`] so glyph runs carry the
/// same paint vocabulary as [`forgekit_scene`].
pub struct TextContext {
    font_ctx: parley::FontContext,
    layout_ctx: parley::LayoutContext<Brush>,
}

impl TextContext {
    /// Builds a context with system fonts available.
    ///
    /// [`parley::FontContext::new`] populates the fontique source collection from
    /// the platform (Core Text on macOS); no manual font registration is
    /// required for [`GenericFamily::SystemUi`] to resolve.
    pub fn new() -> Self {
        Self {
            font_ctx: parley::FontContext::new(),
            layout_ctx: parley::LayoutContext::new(),
        }
    }

    /// Lays out `text` with `style`, wrapping to `max_width` when supplied.
    ///
    /// `max_width` is in the same logical-pixel units as `style.size` (the
    /// layout scale is fixed at `1.0` here — physical-pixel scaling is applied
    /// downstream via the scene transform). Passing `None` produces a single
    /// unwrapped line per hard break in `text`. An empty `text` yields a layout
    /// with no glyph runs and a near-zero size.
    pub fn layout(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> TextLayout {
        // scale = 1.0: lay out in logical pixels; the render tier applies the
        // device scale factor. quantize = true snaps advances for crisp glyphs.
        let mut builder = self
            .layout_ctx
            .ranged_builder(&mut self.font_ctx, text, 1.0, true);

        // SystemUi resolves to the platform UI font (e.g. San Francisco on
        // macOS) with no registration; see the task's verified-parley notes.
        builder.push_default(GenericFamily::SystemUi);
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::Brush(Brush::Solid(style.color)));

        let mut layout = builder.build(text);
        layout.break_all_lines(max_width);
        layout.align(
            parley::layout::Alignment::Start,
            parley::layout::AlignmentOptions::default(),
        );

        TextLayout::new(layout)
    }
}

impl Default for TextContext {
    fn default() -> Self {
        Self::new()
    }
}
