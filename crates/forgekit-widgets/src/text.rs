//! The `Text` widget (spec §6.4): a leaf that lays out and paints a string.
//!
//! [`text`] is the declarative view-fn; it produces a [`TextView`] descriptor
//! that materialises into a retained [`TextWidget`]. The widget shapes its
//! content during the layout pass (via the shared `forgekit_text::TextContext`
//! threaded through [`LayoutCtx`]) and emits the resulting glyph runs during
//! paint.

use forgekit_core::{BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget};
use forgekit_text::{TextContext, TextLayout, TextStyle};
use kurbo::Size;
use peniko::Color;

/// A declarative description of a run of text.
///
/// Content does not read application state in v0 — `app_logic` interpolates the
/// string and hands the finished text in (spec §5). Styling is applied with the
/// [`TextView::size`]/[`TextView::color`] builder methods.
pub struct TextView {
    content: String,
    style: TextStyle,
}

/// Create a text view rendering `content` with default styling (16px, black).
pub fn text(content: impl Into<String>) -> TextView {
    TextView {
        content: content.into(),
        style: TextStyle::default(),
    }
}

impl TextView {
    /// Set the font size, in logical pixels.
    pub fn size(mut self, size: f32) -> Self {
        self.style.size = size;
        self
    }

    /// Set the glyph fill color.
    pub fn color(mut self, color: Color) -> Self {
        self.style.color = color;
        self
    }
}

impl<State: 'static> View<State> for TextView {
    type Element = TextWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TextWidget {
        TextWidget {
            content: self.content.clone(),
            style: self.style,
            layout: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.content != self.content {
            element.content = self.content.clone();
            element.layout = None; // invalidate the cached shaping
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.style != self.style {
            element.style = self.style;
            element.layout = None;
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }
}

/// Retained widget for [`TextView`]: caches the shaped [`TextLayout`] produced
/// during layout for reuse in paint.
pub struct TextWidget {
    content: String,
    style: TextStyle,
    /// `None` until the first layout pass, or after a content/style change
    /// invalidates it.
    layout: Option<TextLayout>,
}

impl Widget for TextWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Wrap to the available width when it is bounded (it is, under the
        // root's window-sized loose constraints); an unbounded max width means
        // "lay out on a single line per hard break".
        let max_width = {
            let w = bc.max().width;
            if w.is_finite() {
                Some(w as f32)
            } else {
                None
            }
        };
        let text_ctx = ctx.text_context::<TextContext>();
        let layout = text_ctx.layout(&self.content, &self.style, max_width);
        let size = bc.constrain(layout.size());
        self.layout = Some(layout);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(ctx.origin()) {
                scene.draw_glyph_run(run);
            }
        }
    }
}
