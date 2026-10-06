//! `drawer`: the bottom-anchored panel with a drag handle (vaul's `Drawer`).
//!
//! Source: `apps/v4/registry/new-york-v4/ui/drawer.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a `vaul`
//! `Drawer` with the same `bg-black/50` overlay as [`crate::dialog`] and a
//! `Content` pinned per `direction`:
//!
//! | direction | class list | port |
//! |---|---|---|
//! | `bottom` | `inset-x-0 bottom-0 h-auto max-h-[80vh] rounded-t-lg border-t` + the handle | content-tall up to 80% of the window, top corners rounded, top edge bordered, handle bar |
//! | `top` | `inset-x-0 top-0 mb-24 max-h-[80vh] rounded-b-lg border-b` | mirrored, no handle |
//! | `right`/`left` | `inset-y-0 <side>-0 w-3/4 border-<other> sm:max-w-sm` | full height, `w-3/4` capped at `max-w-sm`, no rounding, no handle |
//!
//! The handle (`mx-auto mt-4 h-2 w-[100px] rounded-full bg-muted`) is
//! `bottom`-only upstream (`group-data-[vaul-drawer-direction=bottom]:block`),
//! and so it is here. `DrawerContent` carries **no shadow class** — unlike the
//! sheet's `shadow-lg` — and no close button.
//!
//! # Drag-to-close and snap points
//!
//! vaul's whole premise is a draggable sheet, and this port's drawer is one on
//! all four edges: a press on the handle — or anywhere on the panel its content
//! did not take — captures, the panel tracks the pointer along its own axis
//! (`Grabbing` while it does, `Grab` over the handle at rest), and the release
//! commits by position and velocity. Past the midpoint (or on a fast flick
//! toward the edge) the drawer continues into the exit ramp and closes;
//! anything shorter settles back open. A press that never moves is still a tap:
//! on the handle it dismisses, which is the button behavior this port shipped
//! before the gesture existed.
//!
//! [`DrawerView::snap_points`] adds Base UI's intermediate resting points — a
//! fraction of the drawer's extent below `1`, logical px above it. The drawer
//! opens at the first point and a release snaps to the nearest one (closing
//! counts as a point of its own), with a flick taking the next one along.
//! [`crate::overlay::modal`] owns the mechanics for both.
//!
//! `drag` is unconditionally on ([`config`] sets it regardless of whether
//! [`DrawerView::on_dismiss`]/[`DrawerView::on_close`] is wired) — matching
//! vaul, which never gates the gesture itself on a close callback existing.
//! What *is* gated is the outcome: a drawer with neither hook wired still
//! drags, but a release past the closing threshold springs back open instead
//! of closing (`crate::overlay::modal`'s dismissable-only exit-ramp rule),
//! the same reachability floor Escape already applied.
//!
//! A controlled `active_snap_point` (with its `on_snap_change` companion) is
//! **not** in this pass: the drawer owns its resting point internally, and an
//! app that needs to drive one from its own state has no seam for it yet.
//!
//! # Slots
//!
//! Same child-composing shape as [`crate::dialog`]'s, with the drawer's own
//! metrics: the panel column has **no gap** (`DrawerContent` sets none — each
//! slot's `p-4` does the spacing), the header is `gap-1.5` and the footer a
//! stretched `gap-2` column. A `bottom` drawer's content is inset by the
//! handle's own strip ([`crate::overlay::HANDLE_RESERVE`]), since the handle is
//! painted as panel chrome rather than laid out as a child.
//!
//! Upstream centres a bottom/top drawer's header text below the `md` breakpoint
//! (`md:text-left`); this port renders the desktop arm and keeps it
//! start-aligned, like every other slot in the catalog.

use std::rc::Rc;

use frust::authoring::{AnyView, BuildCtx, ChangeFlags, View};
use frust::{EdgeInsets, NavigatorController, PopResult};

use crate::overlay::modal::{DRAWER_MAX_HEIGHT_FRACTION, HANDLE_RESERVE};
use crate::overlay::{
    ModalConfig, ModalContent, ModalCorners, ModalExtent, ModalLimit, ModalView, ModalWidget,
    OverlaySide, modal, panel_description, panel_title, stack_slots,
};

/// The gap between the panel's slots: `DrawerContent` sets none.
const SLOT_GAP: f64 = 0.0;
/// `p-4` — the padding each slot carries.
const SLOT_PAD: f64 = 16.0;
/// `md:gap-1.5` — the gap between a header's title and description.
const HEADER_GAP: f64 = 6.0;
/// `gap-2` — the gap between footer children.
const FOOTER_GAP: f64 = 8.0;
/// The drawer title's type size, in logical px: `DrawerTitle` sets weight and
/// ink only (`font-semibold text-foreground`), inheriting `text-base`.
const TITLE_SIZE: f64 = 16.0;

/// The window edge a drawer is pinned to — vaul's `direction` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawerSide {
    /// `inset-x-0 bottom-0 h-auto max-h-[80vh] rounded-t-lg border-t`, with the
    /// drag handle — the default and the only handled side.
    #[default]
    Bottom,
    /// `inset-x-0 top-0 max-h-[80vh] rounded-b-lg border-b`.
    Top,
    /// `inset-y-0 right-0 w-3/4 border-l sm:max-w-sm`.
    Right,
    /// `inset-y-0 left-0 w-3/4 border-r sm:max-w-sm`.
    Left,
}

impl DrawerSide {
    /// The overlay-host side this maps to.
    pub fn overlay_side(self) -> OverlaySide {
        match self {
            DrawerSide::Bottom => OverlaySide::Bottom,
            DrawerSide::Top => OverlaySide::Top,
            DrawerSide::Right => OverlaySide::Right,
            DrawerSide::Left => OverlaySide::Left,
        }
    }

    /// Whether this side shows the drag handle (`bottom` only, per the source).
    pub fn has_handle(self) -> bool {
        self == DrawerSide::Bottom
    }

    /// The corners this side rounds (`rounded-t-lg` / `rounded-b-lg`, square on
    /// a side drawer).
    fn corners(self) -> ModalCorners {
        match self {
            DrawerSide::Bottom => ModalCorners::Top,
            DrawerSide::Top => ModalCorners::Bottom,
            _ => ModalCorners::None,
        }
    }

    /// The content inset this side reserves for the handle strip.
    fn content_pad(self) -> EdgeInsets {
        EdgeInsets {
            left: 0.0,
            top: if self.has_handle() {
                HANDLE_RESERVE
            } else {
                0.0
            },
            right: 0.0,
            bottom: 0.0,
        }
    }
}

/// The drawer panel's chrome for `side`: [`ModalConfig::edge`]'s pinning, with
/// the drawer's own corners, `max-h-[80vh]` cap, absent shadow, handle, and
/// vaul's drag gesture on every edge.
fn config(side: DrawerSide) -> ModalConfig {
    let mut config = ModalConfig::edge(side.overlay_side())
        .corners(side.corners())
        .shadow(None)
        .handle(side.has_handle())
        .drag(true);
    if side.overlay_side().is_vertical() {
        config = config.extent(
            ModalExtent::Content,
            ModalLimit::Fraction(DRAWER_MAX_HEIGHT_FRACTION),
        );
    }
    config
}

/// Create a drawer pinned to `side`, its panel stacking `children` with each
/// slot's own `p-4`.
///
/// `side` is a constructor argument rather than a builder call because it
/// decides the panel's content inset (a `bottom` drawer reserves the handle
/// strip), which is baked into the composed children — vaul takes `direction`
/// on the root for the same reason.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn drawer<State: 'static, V: View<State>>(
    side: DrawerSide,
    children: impl IntoIterator<Item = V>,
) -> DrawerView<State> {
    DrawerView {
        inner: modal(
            stack_slots(children, SLOT_GAP, side.content_pad()),
            config(side),
        ),
    }
}

/// A header slot: a `p-4 gap-1.5` stack (title, description).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn drawer_header<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    stack_slots(children, HEADER_GAP, EdgeInsets::all(SLOT_PAD))
}

/// A title row: `font-semibold text-foreground`.
pub fn drawer_title<State: 'static>(title: impl Into<String>) -> AnyView<State> {
    panel_title(title, TITLE_SIZE)
}

/// A description row: `text-sm text-muted-foreground`.
pub fn drawer_description<State: 'static>(description: impl Into<String>) -> AnyView<State> {
    panel_description(description)
}

/// A footer slot: a stretched `p-4 gap-2` column (upstream's `mt-auto` is not
/// modelled — see [`crate::sheet`]'s note).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn drawer_footer<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    stack_slots(children, FOOTER_GAP, EdgeInsets::all(SLOT_PAD))
}

/// A declarative shadcn drawer. See the [module docs](self).
pub struct DrawerView<State: 'static> {
    inner: ModalView<State>,
}

impl<State: 'static> DrawerView<State> {
    /// Label the drawer's accessibility node — its title text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Set the **unstaged** dismiss callback — a scrim tap, the drag handle, or
    /// Escape — used only when no [`on_close`](Self::on_close) hook is wired
    /// (see [`crate::overlay::modal`]'s exit-motion contract).
    /// [`show_drawer`] wires both.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.inner = self.inner.on_dismiss(on_dismiss);
        self
    }

    /// Set the state-free close hook fired once the exit ramp has settled —
    /// what a `Stack`-mounted drawer wires instead of `on_dismiss` to animate
    /// out. [`show_drawer`] wires this to `controller.pop()`.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }

    /// Set the drawer's snap points: a fraction of its extent at or below `1`,
    /// logical px above it (see the [module docs](self)). The drawer opens at
    /// the first point.
    pub fn snap_points(mut self, points: &[f64]) -> Self {
        self.inner = self.inner.snap_points(points);
        self
    }
}

impl<State: 'static> View<State> for DrawerView<State> {
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

impl<State: 'static> ModalContent<State> for DrawerView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.inner = self.inner.on_modal_dismiss(on_dismiss);
        self
    }
}

/// Push `build`'s drawer as a transparent navigator page and register
/// `on_result` for the value it pops with — [`crate::show_dialog`]'s shape, over
/// the drawer chrome.
pub fn show_drawer<State, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    B: Fn() -> DrawerView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    crate::overlay::show_modal(controller, build, on_result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::modal::tests::{Block, Recorder, WINDOW, escape, ft_ms, pointer};
    use frust::Brightness;
    use frust::authoring::{
        BoxConstraints, CursorIcon, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, Point,
        PointerButton, PointerEvent, PointerPhase, Size, Widget, any, text::TextContext,
    };
    use frust_core::RenderRoot;
    use std::any::Any;
    use std::cell::Cell;

    #[derive(Default)]
    struct Flags {
        dismissed: u32,
    }

    fn sample(side: DrawerSide) -> DrawerView<Flags> {
        drawer(
            side,
            vec![
                drawer_header(vec![
                    drawer_title("Move goal"),
                    drawer_description("Set your daily activity goal."),
                ]),
                drawer_footer(vec![any(Block(Size::new(80.0, 36.0)))]),
            ],
        )
        .on_dismiss(|s: &mut Flags| s.dismissed += 1)
    }

    fn build(view: &DrawerView<Flags>) -> ModalWidget {
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

    /// The same event on the secondary (right) button.
    fn secondary(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    /// [`dispatch`], also reporting whether the event was consumed and whether
    /// the widget asked for the pointer.
    fn dispatch_probe(
        w: &mut ModalWidget,
        state: &mut Flags,
        event: &InputEvent,
    ) -> (EventResult, bool) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        let result = w.event(&mut ctx, event);
        (result, ctx.is_pointer_captured())
    }

    #[test]
    fn a_bottom_drawer_is_content_tall_capped_at_eighty_percent_and_flush() {
        let mut w = build(&sample(DrawerSide::Bottom));
        layout(&mut w);
        settle(&mut w);
        let panel = w.panel_rect();
        assert_eq!(panel.width(), WINDOW.width);
        assert_eq!(panel.y1, WINDOW.height);
        assert!(panel.height() <= WINDOW.height * DRAWER_MAX_HEIGHT_FRACTION);
        // The content clears the handle strip.
        let mut content = Point::ORIGIN;
        Widget::visit_children(&w, &mut |pod| content = pod.origin());
        assert_eq!(content.y, panel.y0);
        assert!(panel.height() > HANDLE_RESERVE);
    }

    #[test]
    fn only_a_bottom_drawer_shows_the_handle() {
        for side in [DrawerSide::Top, DrawerSide::Left, DrawerSide::Right] {
            let mut w = build(&sample(side));
            layout(&mut w);
            assert!(w.handle_rect().is_none(), "{side:?} shows no handle");
            assert!(!side.has_handle());
        }
        let mut w = build(&sample(DrawerSide::Bottom));
        layout(&mut w);
        settle(&mut w);
        let handle = w.handle_rect().expect("a bottom drawer has a handle");
        assert!(
            (handle.center().x - WINDOW.width / 2.0).abs() < 1e-9,
            "mx-auto"
        );
        assert_eq!(handle.width(), 100.0, "`w-[100px]`");
    }

    #[test]
    fn the_handle_dismisses_on_release_and_so_do_the_scrim_and_escape() {
        let mut w = build(&sample(DrawerSide::Bottom));
        layout(&mut w);
        settle(&mut w);
        let mut state = Flags::default();
        let handle = w.handle_rect().unwrap().center();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, handle.x, handle.y),
        );
        assert_eq!(state.dismissed, 0, "never on down");
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, handle.x, handle.y),
        );
        assert_eq!(state.dismissed, 1);

        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 2, "the scrim above the panel dismisses");

        dispatch(&mut w, &mut state, &escape());
        assert_eq!(state.dismissed, 3);
    }

    #[test]
    fn a_drawer_with_no_dismiss_hook_springs_back_from_a_drag_instead_of_wedging() {
        // Neither `on_dismiss` nor `on_close` wired — `drawer` alone, not
        // `sample` (which wires `on_dismiss`).
        let mut w = build(&drawer(
            DrawerSide::Bottom,
            vec![drawer_header(vec![drawer_title("Move goal")])],
        ));
        layout(&mut w);
        settle(&mut w);
        let extent = w.panel_rect().height();
        let start = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + extent * 0.9),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, start.x, start.y + extent * 0.9),
        );
        for ms in [3000.0, 5000.0] {
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
            w.paint(&mut ctx, &mut Recorder::default());
            layout(&mut w);
        }
        assert!(
            (w.progress() - 1.0).abs() < 1e-9,
            "springs back open rather than sticking invisible near zero: {}",
            w.progress()
        );
        assert_eq!(state.dismissed, 0);

        // The panel is still hittable and interactive after the recovery — a
        // fresh handle press still captures normally, not lost to a stuck
        // capture flag left over from the sprung-back drag.
        let (result, captured) = dispatch_probe(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        assert_eq!(result, EventResult::Handled);
        assert!(captured, "the handle still captures a fresh press");
    }

    #[test]
    fn a_secondary_press_is_swallowed_but_drags_and_dismisses_nothing() {
        let mut w = build(&sample(DrawerSide::Bottom));
        layout(&mut w);
        settle(&mut w);
        let mut state = Flags::default();
        let handle = w.handle_rect().unwrap().center();

        let (result, captured) = dispatch_probe(
            &mut w,
            &mut state,
            &secondary(PointerPhase::Down, handle.x, handle.y),
        );
        assert_eq!(
            result,
            EventResult::Handled,
            "the barrier still blocks the page behind it, whatever button pressed"
        );
        assert!(!captured, "but it opens no drag and takes no capture");
        dispatch(
            &mut w,
            &mut state,
            &secondary(PointerPhase::Move, handle.x, handle.y + 60.0),
        );
        assert_eq!(w.progress(), 1.0, "the panel never followed the pointer");
        dispatch(
            &mut w,
            &mut state,
            &secondary(PointerPhase::Up, handle.x, handle.y),
        );
        assert_eq!(state.dismissed, 0);

        // The scrim swallows a secondary press the same way, and dismisses on
        // neither half of it.
        let (result, _) =
            dispatch_probe(&mut w, &mut state, &secondary(PointerPhase::Down, 5.0, 5.0));
        assert_eq!(result, EventResult::Handled);
        dispatch(&mut w, &mut state, &secondary(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 0);

        // The primary gesture still dismisses off the handle.
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, handle.x, handle.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, handle.x, handle.y),
        );
        assert_eq!(state.dismissed, 1);
    }

    /// The `(axis, closing direction)` a drawer on `side` drags along: `+1`
    /// when leaving means growing coordinates.
    fn close_dir(side: DrawerSide) -> (bool, f64) {
        match side {
            DrawerSide::Bottom => (true, 1.0),
            DrawerSide::Top => (true, -1.0),
            DrawerSide::Right => (false, 1.0),
            DrawerSide::Left => (false, -1.0),
        }
    }

    #[test]
    fn every_edge_enters_flush_and_drags_out_to_close() {
        for side in [
            DrawerSide::Bottom,
            DrawerSide::Top,
            DrawerSide::Right,
            DrawerSide::Left,
        ] {
            let closed = Rc::new(Cell::new(0u32));
            let hook = closed.clone();
            let mut w = build(&sample(side).on_close(move || hook.set(hook.get() + 1)));
            layout(&mut w);
            settle(&mut w);
            let panel = w.panel_rect();
            // Flush against its own edge once the slide is over, with the
            // rounding that edge calls for (`rounded-t-lg` / `rounded-b-lg`,
            // square on a side drawer).
            match side {
                DrawerSide::Bottom => {
                    assert_eq!(panel.y1, WINDOW.height);
                    assert_eq!(side.corners(), ModalCorners::Top);
                }
                DrawerSide::Top => {
                    assert_eq!(panel.y0, 0.0);
                    assert_eq!(side.corners(), ModalCorners::Bottom);
                }
                DrawerSide::Right => {
                    assert_eq!(panel.x1, WINDOW.width);
                    assert_eq!(side.corners(), ModalCorners::None);
                }
                DrawerSide::Left => {
                    assert_eq!(panel.x0, 0.0);
                    assert_eq!(side.corners(), ModalCorners::None);
                }
            }

            // Drag from the panel's own surface, most of the way out.
            let (vertical, dir) = close_dir(side);
            let extent = if vertical {
                panel.height()
            } else {
                panel.width()
            };
            let start = panel.center();
            let travel = dir * extent * 0.7;
            let (dx, dy) = if vertical {
                (0.0, travel)
            } else {
                (travel, 0.0)
            };
            let mut state = Flags::default();
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Down, start.x, start.y),
            );
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Move, start.x + dx, start.y + dy),
            );
            assert!(
                (w.progress() - 0.3).abs() < 1e-9,
                "{side:?} tracks the pointer: {}",
                w.progress()
            );
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Up, start.x + dx, start.y + dy),
            );
            for ms in [3000.0, 5000.0] {
                let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
                w.paint(&mut ctx, &mut Recorder::default());
                layout(&mut w);
            }
            assert_eq!(closed.get(), 1, "{side:?} closed");
            assert_eq!(w.progress(), 0.0);
            assert_eq!(state.dismissed, 0, "the staged hook, not the raw callback");
        }
    }

    #[test]
    fn the_cursor_is_grab_over_the_handle_and_grabbing_while_dragging() {
        let mut root: RenderRoot<Flags, DrawerView<Flags>> = RenderRoot::new();
        let mut state = Flags::default();
        let mut logic = |_: &mut Flags| sample(DrawerSide::Bottom);
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        // Two frames to settle the slide, relaying out between them.
        for ms in [0.0, 2000.0] {
            root.paint(&mut Recorder::default(), ft_ms(ms));
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        }
        // Where the handle lands, measured off an identically laid-out widget
        // (the root hands out no widget reference of its own).
        let handle = {
            let mut probe = build(&sample(DrawerSide::Bottom));
            layout(&mut probe);
            settle(&mut probe);
            probe.handle_rect().expect("a handle").center()
        };
        root.event(&mut state, &pointer(PointerPhase::Move, handle.x, handle.y));
        assert_eq!(root.cursor(), CursorIcon::Grab);

        root.event(&mut state, &pointer(PointerPhase::Down, handle.x, handle.y));
        root.event(
            &mut state,
            &pointer(PointerPhase::Move, handle.x, handle.y + 60.0),
        );
        assert_eq!(root.cursor(), CursorIcon::Grabbing);
    }

    #[test]
    fn the_panel_paints_a_muted_handle_and_no_shadow() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let mut w = build(&sample(DrawerSide::Bottom));
        layout(&mut w);
        settle(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(3000.0)).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert!(rec.shadows.is_empty(), "`DrawerContent` carries no shadow");
        let (_, size, radius, color) = rec.rrects[0];
        assert_eq!(size, Size::new(100.0, 8.0), "`h-2 w-[100px]`");
        assert_eq!(radius, 4.0, "`rounded-full`");
        assert_eq!(
            color,
            theme.scheme().surface_container_highest,
            "`bg-muted`"
        );
    }
}
