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
//! the `OverlaySlot`-registered panel are [`crate::tooltip`]'s (read that module
//! for why a hover-opened overlay cannot be an [`crate::overlay::anchored`] host
//! and why `on_open_change` is a best-effort notification), and the panel is
//! [`crate::popover`]'s shared chrome at `w-64`.
//!
//! Three things differ from the tooltip: the open delay is paired with a
//! **close delay** ([`HOVER_CARD_CLOSE_DELAY_MS`], Radix's own default), which
//! is what lets the pointer travel from the trigger onto the panel without the
//! card vanishing under it; the panel carries no arrow tip; and the panel is
//! registered [`frust::OverlayInput::Interactive`] rather than `Transparent`
//! — its content (a link, a button) can genuinely be pressed, which the
//! catalog's hover-driven panels could not do before this port.
//!
//! The panel keeps the card open while the pointer is over it — the registered
//! pod records that in the shared latch, and the trigger treats it as hover of
//! its own.
//!
//! Because the owner keeps registering the panel through its own exit grace
//! window, the card gets its `data-[state=closed]:fade-out-0` exit for free —
//! see [`crate::tooltip`]'s exit-ramp section, which owns the whole ramp for
//! both components.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{BuildCtx, ChangeFlags, OverlayAlign, OverlaySide, View};

use crate::components::popover::{POPOVER_PADDING, PanelHandle, PanelStyle, panel};
use crate::components::tooltip::{
    HOVER_CARD_CLOSE_DELAY_MS, TooltipHover, TooltipLayerView, TooltipLayerWidget,
    TooltipTriggerView, tooltip_layer, tooltip_trigger,
};
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
            // The layer above composites the card's whole fade, in both
            // directions, so the panel runs no ramp of its own and is present
            // for every frame the layer paints it on.
            entrance: false,
            open: true,
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

/// Build a hover-card panel showing `content` while `hover` is open.
///
/// Mount it unconditionally, as a child of the same full-area layered host
/// as before (e.g. [`frust::Stack`]) — it registers nothing with the root
/// while closed, and its content is interactive once open (see the module
/// docs).
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
    type Element = TooltipLayerWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipLayerWidget<State> {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipLayerWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut TooltipLayerWidget<State>, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::POPOVER_PADDING;
    use crate::components::popover::tests::{Recorder, WINDOW, ft_ms, light, pointer};
    use crate::components::tooltip::TOOLTIP_DELAY_MS;
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BoxConstraints, EventCtx, EventResult, InputEvent, LayoutCtx, OverlayPlacement, PaintCtx,
        PaintScene, Point, PointerPhase, Rect, SemanticsCtx, Size, Widget, place, visit_children,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        opens: Vec<bool>,
        link_pressed: bool,
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
                frust::stack()
                    .child(
                        hover_card_trigger(
                            &hover,
                            SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                        )
                        .on_open_change(|s: &mut AppState, open| s.opens.push(open)),
                    )
                    .child(hover_card(
                        &hover,
                        SizedBox(Some(CONTENT.width), Some(CONTENT.height)),
                    ))
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
    fn the_card_fades_out_once_its_close_delay_runs_down() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 10.0, 10.0));
        h.frame(0.0);
        h.frame(TOOLTIP_DELAY_MS as f64);
        let settled = TOOLTIP_DELAY_MS as f64 + 400.0;
        h.paint_at(settled);

        // Off both the trigger and the panel, then past the grace period.
        h.event(pointer(PointerPhase::Move, 380.0, 560.0));
        h.frame(settled);
        let closed = settled + HOVER_CARD_CLOSE_DELAY_MS as f64 + 16.0;
        h.frame(closed);
        assert!(!h.hover.is_open(), "the grace period is over");

        let theme = light();
        let bg = |rec: &Recorder| {
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().surface_container_high)
        };
        let mid = h.paint_at(closed + 100.0);
        assert!(bg(&mid), "the card is still on screen through the fade");
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);
        assert!(
            mid.transforms.is_empty(),
            "a plain fade-out, like the tooltip's"
        );
        let gone = h.paint_at(closed + 400.0);
        assert!(!bg(&gone), "and gone at settle");
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

    /// A probe filling whatever area it is given, recording whether it was
    /// pressed — the content a hover card floats.
    struct LinkProbe;
    struct LinkProbeWidget;

    impl View<AppState> for LinkProbe {
        type Element = LinkProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> LinkProbeWidget {
            LinkProbeWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut LinkProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
        fn teardown(&self, _element: &mut LinkProbeWidget, _ctx: &mut BuildCtx<'_>) {}
    }

    impl Widget for LinkProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            // A fixed size, not `bc.max()`: the card's own panel hands its
            // content a loose box with an *infinite* max height (the same
            // shrink-wrap every popover-chrome panel in this catalog uses), so
            // filling `bc.max()` here would make the card infinitely tall.
            bc.constrain(Size::new(80.0, 24.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.state_mut::<AppState>().link_pressed = true;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
        fn semantics(&self, _ctx: &mut SemanticsCtx) {}
        visit_children!();
    }

    /// The capability this port adds: a hover card's own content is now
    /// genuinely interactive (`OverlayInput::Interactive`), where the old
    /// layer's `event()` always returned `Ignored` unconditionally and never
    /// routed to its content at all — this could not have fired before the
    /// port, by construction of the old code (see the module docs).
    #[test]
    fn a_press_inside_the_open_cards_own_content_reaches_it() {
        let hover = TooltipHover::new();
        let mut state = AppState::default();
        let mut root: RenderRoot<AppState, frust::StackView<AppState>> = RenderRoot::new();
        root.set_theme(Box::new(light()));
        let mut tcx = TextContext::new();

        let mut run = |root: &mut RenderRoot<AppState, frust::StackView<AppState>>,
                       state: &mut AppState,
                       ms: f64| {
            let hover = hover.clone();
            let mut logic = move |_s: &mut AppState| {
                frust::stack()
                    .child(hover_card_trigger(
                        &hover,
                        SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                    ))
                    .child(hover_card(&hover, LinkProbe))
            };
            root.rebuild(&mut logic, state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft_ms(ms));
        };

        run(&mut root, &mut state, 0.0);
        root.event(&mut state, &pointer(PointerPhase::Move, 10.0, 10.0));
        run(&mut root, &mut state, 0.0);
        run(&mut root, &mut state, TOOLTIP_DELAY_MS as f64);
        assert!(hover.is_open());

        // The card's own content sits inset by `POPOVER_PADDING` inside the
        // placed panel — landing on the panel's own rect is not enough (that
        // padding border is chrome, not `LinkProbe`), so this computes where
        // the content itself sits, the same way `PanelWidget::layout` does.
        let content_size = Size::new(80.0, 24.0);
        let panel = place(
            hover.anchor(),
            Size::new(
                HOVER_CARD_WIDTH,
                content_size.height + 2.0 * POPOVER_PADDING,
            ),
            Rect::from_origin_size(Point::ORIGIN, WINDOW),
            OverlayPlacement::default(),
        );
        let inside_content = Point::new(
            panel.x0 + POPOVER_PADDING + content_size.width / 2.0,
            panel.y0 + POPOVER_PADDING + content_size.height / 2.0,
        );
        // A press landing inside a registered overlay is routed to it as
        // `InputEvent::Overlay`, which the main tree's own dispatch treats as
        // a broadcast — "never consumed, whatever the pod returned"
        // (`OverlaySlot::route`'s own doc) — so the top-level `EventOutcome`
        // never reflects a pod's result by design; `state.link_pressed` is
        // the reliable proof that the press actually reached the content.
        root.event(
            &mut state,
            &pointer(PointerPhase::Down, inside_content.x, inside_content.y),
        );
        assert!(
            state.link_pressed,
            "the press reaches the card's own content"
        );
    }
}
