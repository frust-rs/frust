//! `hover_card`: the popover-chromed panel a hover opens after a delay.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/hover-card.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `HoverCard` portalled to the body at `sideOffset={4}`, whose content is
//! `z-50 w-64 rounded-md border bg-popover p-4 text-popover-foreground shadow-md`
//! entering with `fade-in-0 zoom-in-95`.
//!
//! # Shape of the port
//!
//! A hover card is the tooltip's mechanics wearing the popover's clothes, and
//! that is exactly how it is built: the hover latch, the frame-clock delays and
//! the input-transparent top layer are [`crate::tooltip`]'s (read that module for
//! why a hover-opened overlay cannot be an [`crate::overlay::anchored`] host and
//! why `on_open_change` is a best-effort notification), and the panel is
//! [`crate::popover`]'s shared chrome at `w-64`.
//!
//! Two things differ from the tooltip: the open delay is paired with a **close
//! delay** ([`HOVER_CARD_CLOSE_DELAY_MS`], Radix's own default), which is what
//! lets the pointer travel from the trigger onto the panel without the card
//! vanishing under it; and the panel carries no arrow tip.
//!
//! The panel keeps the card open while the pointer is over it — the layer records
//! that in the shared latch, and the trigger treats it as hover of its own.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{BuildCtx, ChangeFlags, View};

use crate::components::popover::{POPOVER_PADDING, PanelHandle, PanelStyle, panel};
use crate::components::tooltip::{
    HOVER_CARD_CLOSE_DELAY_MS, TooltipHover, TooltipLayerView, TooltipLayerWidget,
    TooltipTriggerView, tooltip_layer, tooltip_trigger,
};
use crate::overlay::{OverlayAlign, OverlaySide};
use crate::style;

/// `w-64` — the hover card's panel width, in logical px.
pub const HOVER_CARD_WIDTH: f64 = 256.0;

impl PanelStyle {
    /// `w-64 p-4 shadow-md` — the hover card's own chrome.
    pub(crate) fn hover_card() -> Self {
        PanelStyle {
            pad_x: POPOVER_PADDING,
            pad_y: POPOVER_PADDING,
            width: Some(HOVER_CARD_WIDTH),
            min_width: 0.0,
            anchor: None,
            shadow: style::SHADOW_MD,
            entrance: false,
        }
    }
}

/// Wrap `child` as a hover-card trigger writing into `hover`.
///
/// The tooltip's trigger with the card's own delays: it opens after
/// [`crate::TOOLTIP_DELAY_MS`] of hover or on focus, and closes
/// [`HOVER_CARD_CLOSE_DELAY_MS`] after the pointer leaves both it and the panel.
pub fn hover_card_trigger<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    child: V,
) -> TooltipTriggerView<State> {
    tooltip_trigger(hover, child).close_delay(Duration::from_millis(HOVER_CARD_CLOSE_DELAY_MS))
}

/// Build a hover-card layer showing `content` while `hover` is open.
///
/// Mount it as the top child of a full-area [`frust::Stack`], permanently: it
/// paints nothing while closed and never consumes input.
pub fn hover_card<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    content: V,
) -> HoverCardView<State> {
    let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::hover_card()));
    HoverCardView {
        inner: tooltip_layer(hover, panel(content, style)),
    }
}

/// A declarative shadcn hover card. See [`hover_card`].
pub struct HoverCardView<State: 'static> {
    inner: TooltipLayerView<State>,
}

impl<State: 'static> HoverCardView<State> {
    /// Set the side the card opens on (`side`, default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.inner = self.inner.side(side);
        self
    }

    /// Set the cross-axis alignment (`align`, default `center`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.inner = self.inner.align(align);
        self
    }

    /// Set the gap between trigger and card (`sideOffset`, default `4`).
    pub fn offset(mut self, offset: f64) -> Self {
        self.inner = self.inner.offset(offset);
        self
    }
}

impl<State: 'static> View<State> for HoverCardView<State> {
    type Element = TooltipLayerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipLayerWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipLayerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut TooltipLayerWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, ft_ms, light, pointer};
    use crate::components::tooltip::TOOLTIP_DELAY_MS;
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{InputEvent, Point, PointerPhase, Size, any};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        opens: Vec<bool>,
    }

    const TRIGGER: Size = Size::new(80.0, 36.0);
    const CONTENT: Size = Size::new(120.0, 60.0);

    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        hover: TooltipHover,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                hover: TooltipHover::new(),
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        fn frame(&mut self, ms: f64) {
            let hover = self.hover.clone();
            let mut logic = move |_s: &mut AppState| {
                frust::Stack(vec![
                    any(hover_card_trigger(
                        &hover,
                        SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                    )
                    .on_open_change(|s: &mut AppState, open| s.opens.push(open))),
                    any(hover_card(
                        &hover,
                        SizedBox(Some(CONTENT.width), Some(CONTENT.height)),
                    )),
                ])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            self.root.paint(&mut Recorder::default(), ft_ms(ms));
        }

        fn paint_at(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }
    }

    #[test]
    fn the_card_opens_on_the_same_delay_the_tooltip_uses() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        assert!(!h.hover.is_open());
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());
    }

    #[test]
    fn the_close_delay_keeps_it_up_while_the_pointer_crosses_to_the_panel() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        assert!(h.hover.is_open());

        // The pointer leaves the trigger: the card survives the grace period…
        let left = TOOLTIP_DELAY_MS as f64;
        h.event(pointer(PointerPhase::Move, 300.0, 300.0));
        h.frame(left + 16.0);
        assert!(h.hover.is_open(), "still inside the close delay");
        // …and goes on surviving while the pointer is over the panel itself.
        let panel = Point::new(
            TRIGGER.width / 2.0,
            TRIGGER.height + crate::overlay::SIDE_OFFSET + 10.0,
        );
        h.event(pointer(PointerPhase::Move, panel.x, panel.y));
        h.frame(left + HOVER_CARD_CLOSE_DELAY_MS as f64 + 32.0);
        assert!(h.hover.is_open(), "the panel keeps its own card open");

        // Off both: the delay runs out and it closes.
        h.event(pointer(PointerPhase::Move, 380.0, 560.0));
        let gone = left + HOVER_CARD_CLOSE_DELAY_MS as f64 + 48.0;
        h.frame(gone);
        h.frame(gone + HOVER_CARD_CLOSE_DELAY_MS as f64 + 16.0);
        assert!(!h.hover.is_open());
    }

    #[test]
    fn the_open_card_paints_popover_chrome_at_w_64() {
        let mut h = Harness::new();
        assert!(h.paint_at(0.0).rrects.is_empty(), "nothing while closed");
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        let rec = h.paint_at(TOOLTIP_DELAY_MS as f64 + 1000.0);
        let theme = light();
        let (_, size, _, color) = rec
            .rrects
            .iter()
            .copied()
            .find(|(_, _, _, c)| *c == theme.scheme().surface_container_high)
            .expect("a bg-popover panel");
        assert_eq!(size.width, HOVER_CARD_WIDTH, "w-64");
        assert_eq!(color, theme.scheme().surface_container_high);
        assert!(
            rec.shadows
                .iter()
                .any(|(_, _, _, sd, _)| *sd == style::SHADOW_MD.std_dev),
            "shadow-md"
        );
    }
}
