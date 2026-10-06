//! `sheet`: a panel that slides in from a window edge.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/sheet.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `Dialog` with the same `bg-black/50` overlay as [`crate::dialog`] and a
//! `Content` pinned per `side`:
//!
//! | `side` | class list | port |
//! |---|---|---|
//! | `right` | `inset-y-0 right-0 h-full w-3/4 border-l sm:max-w-sm` | full height, `w-3/4` capped at `max-w-sm`, bordered on its left edge |
//! | `left` | `inset-y-0 left-0 h-full w-3/4 border-r sm:max-w-sm` | mirrored |
//! | `top` | `inset-x-0 top-0 h-auto border-b` | full width, content-tall |
//! | `bottom` | `inset-x-0 bottom-0 h-auto border-t` | mirrored |
//!
//! plus `bg-background shadow-lg`, no rounding at all, a `showCloseButton` X at
//! `absolute top-4 right-4`, and `slide-in-from-<side>` at
//! `data-[state=open]:duration-500`. Every one of those is
//! [`crate::overlay::modal`]'s chrome, configured here; this module contributes
//! the side mapping and the slot vocabulary.
//!
//! # No swipe-to-dismiss
//!
//! `SheetContent` is a Radix **dialog**, not vaul's drawer: upstream it has no
//! drag gesture at all, so this port leaves [`crate::overlay::ModalConfig`]'s
//! `drag` off and the panel is dismissed by the scrim, the close X, or Escape
//! only. What it does inherit from the shared host is the **staged exit** —
//! every one of those three reverses the `slide-in-from-<side>` it entered with
//! before the dismissal fires (see [`crate::overlay::modal`]).
//!
//! # Slots
//!
//! Same child-composing shape as [`crate::dialog`]'s (see its module docs), with
//! the sheet's own metrics: the panel column is `gap-4` with **no padding of its
//! own** — each slot carries `p-4` instead — the header is `gap-1.5`, and the
//! footer is a stretched `gap-2` **column**, not a trailing row (the source's
//! `SheetFooter` has no `sm:flex-row`).
//!
//! # Deviation: `mt-auto`
//!
//! Upstream's `SheetFooter` carries `mt-auto`, pinning it to the bottom of a
//! full-height side sheet. This port packs the slots from the leading edge
//! instead — `FlexView` takes its flex factors from the child list, not from a
//! class on a child, and the slot list here is the caller's. A caller who wants
//! the footer pinned gives the content slot the remaining height itself (a
//! `SizedBox`/scroll view sized to it).

use std::rc::Rc;

use frust::authoring::{AnyView, BuildCtx, ChangeFlags, View};
use frust::{EdgeInsets, NavigatorController, PopResult};

use crate::overlay::{
    ModalConfig, ModalContent, ModalView, ModalWidget, OverlaySide, modal, panel_description,
    panel_title, stack_slots,
};

/// `gap-4` — the gap between the panel's slots.
const SLOT_GAP: f64 = 16.0;
/// `p-4` — the padding each slot carries (the panel carries none).
const SLOT_PAD: f64 = 16.0;
/// `gap-1.5` — the gap between a header's title and description.
const HEADER_GAP: f64 = 6.0;
/// `gap-2` — the gap between footer children.
const FOOTER_GAP: f64 = 8.0;
/// The sheet title's type size, in logical px: `SheetTitle` sets weight and ink
/// only (`font-semibold text-foreground`), so it inherits the body's own
/// `text-base`.
const TITLE_SIZE: f64 = 16.0;

/// The window edge a sheet is pinned to — `SheetContent`'s `side` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SheetSide {
    /// `inset-x-0 top-0 h-auto border-b`.
    Top,
    /// `inset-y-0 right-0 h-full w-3/4 border-l sm:max-w-sm` — the default.
    #[default]
    Right,
    /// `inset-x-0 bottom-0 h-auto border-t`.
    Bottom,
    /// `inset-y-0 left-0 h-full w-3/4 border-r sm:max-w-sm`.
    Left,
}

impl SheetSide {
    /// The overlay-host side this maps to.
    pub fn overlay_side(self) -> OverlaySide {
        match self {
            SheetSide::Top => OverlaySide::Top,
            SheetSide::Right => OverlaySide::Right,
            SheetSide::Bottom => OverlaySide::Bottom,
            SheetSide::Left => OverlaySide::Left,
        }
    }
}

/// The sheet panel's chrome for `side` — [`ModalConfig::edge`]'s defaults are
/// already the source's (`w-3/4 sm:max-w-sm` on a side sheet, `h-auto` on a
/// top/bottom one, a square-cornered `shadow-lg` panel bordered on its inner
/// edge, sliding in).
fn config(side: SheetSide, close_button: bool) -> ModalConfig {
    ModalConfig::edge(side.overlay_side()).close_button(close_button)
}

/// Create a sheet whose panel stacks `children` `gap-4` apart, pinned to the
/// window's trailing edge until [`SheetView::side`] says otherwise.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn sheet<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> SheetView<State> {
    let side = SheetSide::default();
    SheetView {
        inner: modal(
            stack_slots(children, SLOT_GAP, EdgeInsets::all(0.0)),
            config(side, true),
        ),
        side,
        close_button: true,
    }
}

/// A header slot: a `p-4 gap-1.5` stack (title, description).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn sheet_header<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    stack_slots(children, HEADER_GAP, EdgeInsets::all(SLOT_PAD))
}

/// A title row: `font-semibold text-foreground`.
pub fn sheet_title<State: 'static>(title: impl Into<String>) -> AnyView<State> {
    panel_title(title, TITLE_SIZE)
}

/// A description row: `text-sm text-muted-foreground`.
pub fn sheet_description<State: 'static>(description: impl Into<String>) -> AnyView<State> {
    panel_description(description)
}

/// A footer slot: a stretched `p-4 gap-2` column (see the [module docs](self)'s
/// `mt-auto` note).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn sheet_footer<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    stack_slots(children, FOOTER_GAP, EdgeInsets::all(SLOT_PAD))
}

/// A declarative shadcn sheet. See the [module docs](self).
pub struct SheetView<State: 'static> {
    inner: ModalView<State>,
    side: SheetSide,
    close_button: bool,
}

impl<State: 'static> SheetView<State> {
    /// Pin the sheet to `side`.
    pub fn side(mut self, side: SheetSide) -> Self {
        self.side = side;
        self.inner.config = config(side, self.close_button);
        self
    }

    /// Show or hide the `top-4 right-4` close X (upstream's `showCloseButton`,
    /// default `true`).
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.close_button = close_button;
        self.inner.config = config(self.side, close_button);
        self
    }

    /// Label the sheet's accessibility node — its title text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Set the **unstaged** dismiss callback — a scrim tap, the close X, or
    /// Escape — used only when no [`on_close`](Self::on_close) hook is wired
    /// (see [`crate::overlay::modal`]'s exit-motion contract). [`show_sheet`]
    /// wires both.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.inner = self.inner.on_dismiss(on_dismiss);
        self
    }

    /// Set the state-free close hook fired once the exit ramp has settled —
    /// what a `Stack`-mounted sheet wires instead of `on_dismiss` to animate
    /// out. [`show_sheet`] wires this to `controller.pop()`.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }
}

impl<State: 'static> View<State> for SheetView<State> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

impl<State: 'static> ModalContent<State> for SheetView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.inner = self.inner.on_modal_dismiss(on_dismiss);
        self
    }
}

/// Push `build`'s sheet as a transparent navigator page and register
/// `on_result` for the value it pops with — [`crate::show_dialog`]'s shape, over
/// the sheet chrome.
pub fn show_sheet<State, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    B: Fn() -> SheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    crate::overlay::show_modal(controller, build, on_result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::modal::tests::{Block, Recorder, WINDOW, escape, ft_ms, pointer};
    use crate::overlay::modal::{EDGE_FRACTION, MAX_WIDTH_SM};
    use frust::authoring::{
        BoxConstraints, EventCtx, InputEvent, LayoutCtx, PaintCtx, Point, PointerPhase, Size,
        Widget, text::TextContext,
    };
    use frust::{Brightness, FrameTime};
    use std::any::Any;
    use std::cell::Cell;

    #[derive(Default)]
    struct Flags {
        dismissed: u32,
    }

    fn sample(side: SheetSide) -> SheetView<Flags> {
        sheet(vec![
            sheet_header(vec![
                sheet_title("Edit profile"),
                sheet_description("Make changes to your profile here."),
            ]),
            sheet_footer(vec![Block(Size::new(80.0, 36.0))]),
        ])
        .side(side)
        .on_dismiss(|s: &mut Flags| s.dismissed += 1)
    }

    fn build(view: &SheetView<Flags>) -> ModalWidget {
        let mut counter = 0u64;
        View::<Flags>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ModalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    /// Paint two frames so the slide is over, relaying out between them.
    fn settle(w: &mut ModalWidget) {
        for ms in [0.0, 2000.0] {
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
            w.paint(&mut ctx, &mut Recorder::default());
            layout(w);
        }
    }

    fn dispatch(w: &mut ModalWidget, state: &mut Flags, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event);
    }

    #[test]
    fn a_side_sheet_is_three_quarters_wide_full_height_and_flush_at_rest() {
        for side in [SheetSide::Left, SheetSide::Right] {
            let mut w = build(&sample(side));
            layout(&mut w);
            settle(&mut w);
            let panel = w.panel_rect();
            assert_eq!(
                panel.width(),
                (WINDOW.width * EDGE_FRACTION).min(MAX_WIDTH_SM)
            );
            assert_eq!(panel.height(), WINDOW.height);
            match side {
                SheetSide::Left => assert_eq!(panel.x0, 0.0),
                _ => assert_eq!(panel.x1, WINDOW.width),
            }
        }
    }

    #[test]
    fn a_top_or_bottom_sheet_is_full_width_and_content_tall() {
        for side in [SheetSide::Top, SheetSide::Bottom] {
            let mut w = build(&sample(side));
            layout(&mut w);
            settle(&mut w);
            let panel = w.panel_rect();
            assert_eq!(panel.width(), WINDOW.width);
            assert!(
                panel.height() > 0.0 && panel.height() < WINDOW.height,
                "`h-auto`: {}",
                panel.height()
            );
            match side {
                SheetSide::Top => assert_eq!(panel.y0, 0.0),
                _ => assert_eq!(panel.y1, WINDOW.height),
            }
        }
    }

    #[test]
    fn the_panel_slides_in_from_its_own_edge() {
        let mut w = build(&sample(SheetSide::Left));
        layout(&mut w);
        // Before the first paint the panel is entirely outside the left edge.
        assert!(w.panel_rect().x1 <= 0.0);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, FrameTime::ZERO);
        w.paint(&mut ctx, &mut Recorder::default());
        layout(&mut w);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(250.0));
        w.paint(&mut ctx, &mut Recorder::default());
        layout(&mut w);
        let mid = w.panel_rect();
        assert!(mid.x0 < 0.0 && mid.x1 > 0.0, "mid-slide: {}", mid.x0);
        settle(&mut w);
        assert_eq!(w.panel_rect().x0, 0.0);
    }

    #[test]
    fn the_panel_is_square_cornered_bordered_on_its_inner_edge_and_shadowed() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let mut w = build(&sample(SheetSide::Right));
        layout(&mut w);
        settle(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(3000.0)).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.rects[0].2, theme.scheme().scrim);
        assert_eq!(rec.paths[0].2, theme.scheme().surface);
        assert_eq!(rec.shadows[0].3, crate::style::SHADOW_LG.std_dev);
        // One border stroke, along the panel's left (inner) edge: a zero-width
        // bounding box at the panel's own leading edge.
        let (bbox, width, color) = rec.strokes[0];
        assert_eq!(width, crate::style::BORDER_WIDTH);
        assert_eq!(color, theme.scheme().outline);
        assert!(bbox.width() < 1e-9, "a vertical rule, not a rectangle");
        assert!((bbox.x0 - w.panel_rect().x0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn every_side_reverses_its_own_slide_before_closing_and_none_of_them_drags() {
        for side in [
            SheetSide::Top,
            SheetSide::Right,
            SheetSide::Bottom,
            SheetSide::Left,
        ] {
            let closed = Rc::new(Cell::new(0u32));
            let hook = closed.clone();
            let mut w = build(&sample(side).on_close(move || hook.set(hook.get() + 1)));
            layout(&mut w);
            settle(&mut w);
            let open = w.panel_rect();
            let mut state = Flags::default();

            // A drag on the panel is inert here — the sheet is dialog-family.
            let c = open.center();
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Move, c.x + 80.0, c.y + 80.0),
            );
            assert_eq!(w.panel_rect(), open, "{side:?} does not follow a drag");
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Up, c.x + 80.0, c.y + 80.0),
            );
            assert_eq!(closed.get(), 0, "and a released drag dismisses nothing");

            // Escape stages the exit: the panel slides back out of its edge
            // before the close fires.
            dispatch(&mut w, &mut state, &escape());
            assert_eq!(closed.get(), 0, "deferred behind the ramp");
            let paint = |w: &mut ModalWidget, ms: f64| {
                let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
                w.paint(&mut ctx, &mut Recorder::default());
                layout(w);
            };
            paint(&mut w, 3000.0);
            paint(&mut w, 3000.0 + 250.0);
            let mid = w.panel_rect();
            assert!(
                w.progress() > 0.0 && w.progress() < 1.0,
                "{side:?} mid-exit"
            );
            match side {
                SheetSide::Top => assert!(mid.y0 < open.y0),
                SheetSide::Bottom => assert!(mid.y0 > open.y0),
                SheetSide::Left => assert!(mid.x0 < open.x0),
                SheetSide::Right => assert!(mid.x0 > open.x0),
            }
            paint(&mut w, 5000.0);
            assert_eq!(closed.get(), 1, "{side:?} closed at the settle");
            assert_eq!(state.dismissed, 0);
        }
    }

    #[test]
    fn the_close_x_and_a_scrim_tap_both_dismiss() {
        let mut w = build(&sample(SheetSide::Right));
        layout(&mut w);
        settle(&mut w);
        let mut state = Flags::default();
        let close = w.close_rect().expect("a close box").center();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, close.x, close.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, close.x, close.y),
        );
        assert_eq!(state.dismissed, 1);

        // The scrim is everything outside the panel — here, the window's left.
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 2);
    }
}
