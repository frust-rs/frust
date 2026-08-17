//! Ports shadcn/ui's **Label** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/label.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — `text-sm font-medium
//! leading-none`, with an opacity-50 disabled treatment.
//!
//! # Deviation: `disabled` is an explicit flag, not `group-data`/`peer`
//! selectors
//!
//! Upstream reads its disabled look off a Radix `group`/`peer` CSS relation
//! (`group-data-[disabled=true]:opacity-50 peer-disabled:opacity-50` /
//! `peer-disabled:cursor-not-allowed`) — the label observes whatever control
//! it's paired with, rather than being told directly. This port has no CSS
//! relation to lean on, so [`LabelView::disabled`] is the explicit flag a
//! caller sets to mirror the paired control's own `disabled` state (plain
//! composition, by design — no automatic pairing mechanism is
//! invented). The `cursor-not-allowed` half of that CSS rule has no paint-time
//! counterpart either: a label is not itself hit-tested, so there is no
//! `Move` arm to request a cursor from.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Role,
    SemanticsCtx, Size, View, Widget,
};

use crate::style::{TEXT_SM, disabled_tint};
use crate::text::Label;

/// Unthemed fallback ink (a theme resolves this from `colors.on_surface`,
/// shadcn's `--foreground`).
const FALLBACK_INK: Color = Color::from_rgb8(0x0A, 0x0A, 0x0A);

/// A declarative shadcn label.
pub struct LabelView {
    text: String,
    disabled: bool,
}

/// Create a label reading `text`.
pub fn label(text: impl Into<String>) -> LabelView {
    LabelView {
        text: text.into(),
        disabled: false,
    }
}

impl LabelView {
    /// Apply the paired control's disabled look (50% opacity — see the
    /// [module docs](self)'s deviation note).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The retained widget for a [`LabelView`].
pub struct LabelWidget {
    label: Label,
    text: String,
    disabled: bool,
}

impl<State: 'static> View<State> for LabelView {
    type Element = LabelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LabelWidget {
        LabelWidget {
            label: Label::new(self.text.clone()),
            text: self.text.clone(),
            disabled: self.disabled,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LabelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.text != self.text {
            element.label.set_content(self.text.clone());
            element.text = self.text.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The resolved ink: themed `colors.on_surface`, else [`FALLBACK_INK`].
fn resolve_ink(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface)
}

/// The label's style: `text-sm font-medium`, at the resolved (possibly
/// disabled-dimmed) ink.
fn label_style(ink: Color) -> TextStyle {
    TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TEXT_SM as f32, ink)
    }
}

impl Widget for LabelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let ink = disabled_tint(resolve_ink(theme), self.disabled);
        let style = label_style(ink);
        let size = self.label.layout(ctx, &style);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.label.paint(ctx.origin(), scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Label, |node| {
            node.set_label(self.text.as_str());
            if self.disabled {
                node.set_disabled();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::DISABLED_OPACITY;
    use frust::authoring::Point;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    fn build(view: &LabelView) -> LabelWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut LabelWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    #[derive(Default)]
    struct Recorder {
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let frust::authoring::Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn paint(w: &mut LabelWidget, size: Size) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_ink() {
        let mut w = build(&label("Email"));
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size);
        assert_eq!(rec.glyph_colors[0], FALLBACK_INK);
    }

    #[test]
    fn themed_paint_resolves_on_surface() {
        let theme = crate::tokens::theme();
        let mut w = build(&label("Email"));
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size);
        assert_eq!(rec.glyph_colors[0], theme.scheme().on_surface);
    }

    #[test]
    fn disabled_dims_the_ink_to_the_shared_disabled_opacity() {
        let mut w = build(&label("Email").disabled(true));
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size);
        assert_eq!(rec.glyph_colors[0].components[3], DISABLED_OPACITY);
    }

    #[test]
    fn rebuild_repaints_on_a_disabled_flip() {
        let prev = label("Email");
        let next = label("Email").disabled(true);
        let mut element = build(&prev);
        let mut counter = 0u64;
        let flags =
            View::<()>::rebuild(&next, &prev, &mut element, &mut BuildCtx::new(&mut counter));
        assert_eq!(flags, ChangeFlags::PAINT);
        assert!(element.disabled);
    }

    #[test]
    fn rebuild_relayouts_on_a_text_change() {
        let prev = label("Email");
        let next = label("Password");
        let mut element = build(&prev);
        let mut counter = 0u64;
        let flags =
            View::<()>::rebuild(&next, &prev, &mut element, &mut BuildCtx::new(&mut counter));
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert_eq!(element.text, "Password");
    }
}
