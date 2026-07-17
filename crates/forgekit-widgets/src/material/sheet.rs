//! The modal M3 `BottomSheet` (Phase 6c, PLAN.md D3/D5, task 11): a full-area
//! widget = **scrim** (32% scrim-color fill; tap outside the sheet dismisses) +
//! a **bottom-anchored panel** (top corners extra-large 28dp — ledger R12/R16,
//! `surfaceContainerLow`, a centered drag handle **32dp wide × 4dp tall** in a
//! 48dp touch target — ledger R16), pushed as a transparent navigator page via
//! [`NavigatorController::push_transparent_for_result`] and entering via the
//! [`PageTransition::SlideUp`] preset (task 06).
//!
//! # Navigator-modal architecture (D3)
//!
//! Like [`crate::material::dialog`], the scrim is part of *this* widget (the
//! navigator provides none and routes input only to the top page), and the
//! sheet is designed to be pushed as a transparent page over a static
//! background. [`show_bottom_sheet`] wraps the push. Dismissal is always
//! `controller.pop()`, wired to two gestures:
//!
//! * a **scrim tap** (a press+release outside the panel) pops with an *empty*
//!   [`PopResult`];
//! * a **drag-down on the handle** past a threshold pops.
//!
//! ## Drag-to-dismiss is threshold-simple (v1)
//!
//! The handle-drag dismiss is intentionally *not* a full interactive settle like
//! the navigator's edge-swipe: the sheet does **not** follow the finger frame by
//! frame and does **not** spring back on a short drag. Instead, a press that
//! starts in the handle's 48dp touch strip and releases more than
//! [`DRAG_DISMISS_FRACTION`] of the sheet height lower simply pops; a shorter
//! drag does nothing. A richer interactive settle (mirroring
//! [`crate::NavigatorWidget`]'s held-transition edge-swipe) is deferred.
//!
//! # Panel anatomy (ledger R12/R16)
//!
//! The panel spans the full width, anchored to the bottom edge, with its **top**
//! two corners rounded to the extra-large (28dp) shape and its bottom corners
//! square (they sit at the screen edge). It sizes to its content plus the 48dp
//! handle strip at the top; the visual drag-handle indicator is 32×4dp, centered
//! horizontally within that strip.
//!
//! # Standalone use
//!
//! A `BottomSheet` is a plain widget: it works inside a [`crate::Stack`] too
//! (the scrim is still its own). The navigator path is the primary, documented
//! one. This module does **not** edit any nav file — task 06 owns those.
//!
//! # Semantics
//!
//! The sheet contributes one [`Role::Dialog`] container node with the accesskit
//! **modal** flag set — a documented choice: accesskit has no dedicated
//! "bottom sheet" role, and a modal sheet is, to assistive tech, a modal
//! dialog-ish surface (the same role the [`crate::material::dialog`] uses). Its
//! content becomes the node's accesskit children.
//!
//! # Keyboard operability (task 07, 6d)
//!
//! **Escape-to-dismiss now works, once the sheet has focus.** A `Down`
//! anywhere in the sheet (scrim, handle, or panel background) claims focus
//! via `EventCtx::request_focus` — the sheet already captures its whole area,
//! so this is a pure opt-in with no new hit-testing. Once focused, a
//! focus-routed `Key(Escape)` invokes the *same* dismiss path as a scrim tap
//! or handle drag (`on_dismiss`) — unless the sheet's `content` itself holds
//! the deeper focus path, in which case content gets first crack at the key
//! (mirroring how an action consumes an event before the dialog's modal
//! barrier does). **There is still no hook to focus the sheet on appear**
//! (auto-focus-on-appear) — a caller must complete one pointer interaction
//! with the sheet before Escape does anything; that gap is deferred to a
//! future focus-manager work item, not this task.
//!
//! The accesskit **modal** flag set above is nonetheless **kept deliberately**,
//! not dropped. No platform `accesskit_*` adapter is wired yet (see
//! `docs/ARCHITECTURE.md`'s Semantics pass), so there is no live assistive-tech
//! audience the flag could currently mislead.
//!
//! # State layer (task 07, 6d)
//!
//! This sheet has no `StateLayer` surface of its own (the scrim, panel, and
//! drag handle are plain fills, not an M3 interactive surface) — there is
//! nothing here for `StateLayer::set_focused` to wire into.

use std::rc::Rc;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size};
use peniko::{Brush, Color};

use crate::nav::navigator::{NavigatorController, PopResult};
use crate::nav::transition::{PageTransition, TransitionSpec};

/// Scrim opacity behind a modal bottom sheet (M3 spec: 32%, matching the
/// dialog scrim, ledger R15).
const SCRIM_ALPHA: f32 = 0.32;
/// Unthemed-fallback panel top-corner radius (a theme resolves this from
/// `shape.extra_large`, ledger R12's 28dp token).
const RADIUS: f64 = 28.0;
/// Drag-handle visual indicator width, in logical px (M3 token
/// `m3_comp_sheet_bottom_docked_drag_handle_width`, ledger R16).
const HANDLE_WIDTH: f64 = 32.0;
/// Drag-handle visual indicator height, in logical px (M3 token
/// `m3_comp_sheet_bottom_docked_drag_handle_height`, ledger R16).
const HANDLE_HEIGHT: f64 = 4.0;
/// Drag-handle interactive touch-target strip height, in logical px (M3
/// enforces a 48dp minimum touch target on the handle, ledger R16). The whole
/// full-width strip at the top of the sheet is the drag zone.
const HANDLE_TOUCH_TARGET: f64 = 48.0;
/// Flattening tolerance for the top-rounded panel path (a visually-lossless
/// value for on-screen corner radii — the [`crate::material::card`] precedent).
const PATH_TOLERANCE: f64 = 0.1;

/// Fraction of the sheet's own height a handle-drag must exceed to dismiss.
///
/// **Community-approximate**: Material's bottom-sheet dismiss threshold is a
/// fling/drag heuristic with no single published constant; half the sheet
/// height is the conventional drag-past-the-midpoint commit point (the same
/// 0.5 the navigator's edge-swipe uses).
const DRAG_DISMISS_FRACTION: f64 = 0.5;

/// Unthemed-fallback panel container fill (a theme resolves this from
/// `colors.surface_container_low`, ledger R16).
const CONTAINER: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback drag-handle color (a theme resolves this from
/// `colors.on_surface_variant`, the M3 drag-handle color role).
const HANDLE_COLOR: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback scrim base color (a theme resolves this from
/// `colors.scrim`); applied at [`SCRIM_ALPHA`].
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved scrim fill. Themed: `colors.scrim` at [`SCRIM_ALPHA`].
fn resolve_scrim(theme: Option<&Theme>) -> Color {
    let base = match theme {
        Some(theme) => theme.scheme().scrim,
        None => SCRIM,
    };
    with_alpha(base, SCRIM_ALPHA)
}

/// The resolved panel container fill. Themed: `colors.surface_container_low`.
fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface_container_low,
        None => CONTAINER,
    }
}

/// The resolved drag-handle color. Themed: `colors.on_surface_variant`.
fn resolve_handle(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface_variant,
        None => HANDLE_COLOR,
    }
}

/// The resolved top-corner radius. Themed: `shape.extra_large`.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.extra_large,
        None => RADIUS,
    }
}

/// Coerce a possibly-infinite constraint dimension to a finite value (a sheet
/// expects bounded constraints — a navigator page or a full-screen `Stack`).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// A view-held, typed scrim/drag-dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A declarative modal M3 bottom sheet wrapping a single content child. See the
/// [module docs](self).
pub struct BottomSheetView<State: 'static> {
    content: AnyView<State>,
    on_dismiss: Option<OnDismiss<State>>,
}

/// Wrap `content` in a modal bottom sheet. Chain
/// [`BottomSheetView::on_dismiss`] to handle a dismiss gesture (usually a
/// `controller.pop()` — [`show_bottom_sheet`] wires this for you).
pub fn bottom_sheet<State: 'static, V: View<State>>(content: V) -> BottomSheetView<State> {
    BottomSheetView {
        content: any(content),
        on_dismiss: None,
    }
}

impl<State: 'static> BottomSheetView<State> {
    /// Set the dismiss callback — invoked with `&mut State` on a scrim tap or a
    /// handle drag-down past the threshold. [`show_bottom_sheet`] wires this to
    /// `controller.pop()` automatically.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

/// Push `build`'s sheet as a transparent navigator page (the page below stays
/// visible under the scrim), entering via [`PageTransition::SlideUp`], and
/// register `on_result` for the value the sheet pops with.
///
/// The sheet's dismiss (scrim tap + handle drag) is wired to `controller.pop()`
/// for you (dismiss pops with an *empty* [`PopResult`], overriding any
/// [`BottomSheetView::on_dismiss`] the builder set).
///
/// ```ignore
/// show_bottom_sheet(
///     &state.nav,
///     || bottom_sheet(sheet_content()),
///     |state: &mut State, result: PopResult| { /* ... */ },
/// );
/// ```
pub fn show_bottom_sheet<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> BottomSheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss_ctrl = controller.clone();
    controller.push_transparent_for_result(
        move || {
            let ctrl = dismiss_ctrl.clone();
            any::<State, _>(build().on_dismiss(move |_state: &mut State| ctrl.pop()))
        },
        TransitionSpec::duration(PageTransition::SlideUp),
        on_result,
    );
}

/// The retained widget for a [`BottomSheetView`]. See the [module docs](self).
pub struct BottomSheetWidget {
    content: ChildPod,
    on_dismiss: Option<crate::ErasedCallback>,
    /// The bottom-anchored panel rect in the widget's own local coordinate
    /// space (computed at layout, read for scrim/handle hit-testing at event
    /// time).
    panel: Rect,
    /// The full-width 48dp drag-handle touch strip at the top of the panel
    /// (local coords).
    handle_target: Rect,
    /// A handle drag is in flight (the sheet captured the pointer on a `Down`
    /// in the handle strip).
    drag_active: bool,
    /// The `Down` y the drag distance is measured from.
    drag_start_y: f64,
    /// A scrim/panel-background press is in flight (the modal barrier).
    scrim_captured: bool,
    /// Whether that press started outside the panel (only an outside press
    /// released outside dismisses).
    scrim_down_outside: bool,
}

impl<State: 'static> View<State> for BottomSheetView<State> {
    type Element = BottomSheetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BottomSheetWidget {
        BottomSheetWidget {
            content: crate::build_child(&self.content, ctx),
            on_dismiss: self.on_dismiss.as_ref().map(crate::erase_callback),
            panel: Rect::ZERO,
            handle_target: Rect::ZERO,
            drag_active: false,
            drag_start_y: 0.0,
            scrim_captured: false,
            scrim_down_outside: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BottomSheetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = crate::rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        // Closures aren't comparable — reinstall the dismiss adapter cheaply.
        element.on_dismiss = self.on_dismiss.as_ref().map(crate::erase_callback);
        flags
    }

    fn teardown(&self, element: &mut BottomSheetWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for BottomSheetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        // Content lays out full-width, below the handle strip, in the room left
        // under it.
        let content_max_h = (area_h - HANDLE_TOUCH_TARGET).max(0.0);
        let content_bc = BoxConstraints::loose(Size::new(area_w, content_max_h));
        let content_size = self.content.layout_child(ctx, &content_bc);

        let panel_h = (HANDLE_TOUCH_TARGET + content_size.height).min(area_h);
        let panel_y = area_h - panel_h;
        self.panel = Rect::new(0.0, panel_y, area_w, area_h);
        self.handle_target = Rect::new(0.0, panel_y, area_w, panel_y + HANDLE_TOUCH_TARGET);

        // Center the content horizontally under the handle strip.
        let content_x = ((area_w - content_size.width) / 2.0).max(0.0);
        self.content
            .set_origin(Point::new(content_x, panel_y + HANDLE_TOUCH_TARGET));

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        // Scrim over the whole area.
        scene.fill_rect(ctx.origin(), ctx.size(), resolve_scrim(theme));

        // Panel: full-width, bottom-anchored, top corners rounded and bottom
        // corners square (the bottom sits at the screen edge). No stroked/
        // per-corner rounded-rect primitive exists on `PaintScene`, so build the
        // path with `kurbo` and fill it — the `card` module's precedent.
        let radius = resolve_radius(theme);
        let panel_local = Rect::new(self.panel.x0, self.panel.y0, self.panel.x1, self.panel.y1);
        let radii = RoundedRectRadii::new(radius, radius, 0.0, 0.0);
        let path = RoundedRect::from_rect(panel_local, radii).to_path(PATH_TOLERANCE);
        scene.fill_path(ctx.origin(), &path, &Brush::Solid(resolve_container(theme)));

        // Drag handle: 32×4dp indicator, centered horizontally within the top
        // strip, with fully-rounded ends.
        let handle_x = self.panel.x0 + (self.panel.width() - HANDLE_WIDTH) / 2.0;
        let handle_y = self.panel.y0 + (HANDLE_TOUCH_TARGET - HANDLE_HEIGHT) / 2.0;
        scene.fill_rounded_rect(
            Point::new(ctx.origin().x + handle_x, ctx.origin().y + handle_y),
            Size::new(HANDLE_WIDTH, HANDLE_HEIGHT),
            HANDLE_HEIGHT / 2.0,
            resolve_handle(theme),
        );

        self.content.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // 1. An in-flight handle drag owns the pointer stream.
        if self.drag_active {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            match p.phase {
                PointerPhase::Move => {
                    // Threshold-simple: the sheet does not follow the finger.
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let dy = p.position.y - self.drag_start_y;
                    let threshold = self.panel.height() * DRAG_DISMISS_FRACTION;
                    if dy > threshold
                        && let Some(on_dismiss) = self.on_dismiss.as_mut()
                    {
                        on_dismiss(ctx);
                    }
                    self.drag_active = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // A Cancel never touches state — just clear the drag flag.
                    self.drag_active = false;
                    EventResult::Handled
                }
                PointerPhase::Down => EventResult::Handled,
            }
        }
        // 2. An in-flight scrim/panel-background press owns the stream.
        else if self.scrim_captured {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            match p.phase {
                PointerPhase::Up => {
                    let released_outside = !self.panel.contains(p.position);
                    if self.scrim_down_outside
                        && released_outside
                        && let Some(on_dismiss) = self.on_dismiss.as_mut()
                    {
                        on_dismiss(ctx);
                    }
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                _ => EventResult::Handled,
            }
        }
        // 3. Fresh events.
        else {
            let InputEvent::Pointer(p) = event else {
                // Escape (when the sheet itself, not deeper content, holds the
                // focus path) dismisses through the same path as a scrim/drag
                // dismiss — content gets first crack at it if it holds the
                // deeper focus path (mirrors an action's own routing
                // precedence on `material::dialog`).
                if let InputEvent::Key(key_event) = event
                    && key_event.key == Key::Named(NamedKey::Escape)
                    && !self.content.is_focused()
                    && let Some(on_dismiss) = self.on_dismiss.as_mut()
                {
                    on_dismiss(ctx);
                    return EventResult::Handled;
                }
                // Everything else focus-routed goes to the content.
                return crate::route_event_single(&mut self.content, ctx, event);
            };
            match p.phase {
                PointerPhase::Down => {
                    // A Down anywhere in the sheet claims focus, so a
                    // subsequent Escape has a focus chain to travel.
                    ctx.request_focus();
                    // A Down in the handle strip begins a drag.
                    if self.handle_target.contains(p.position) {
                        self.drag_active = true;
                        self.drag_start_y = p.position.y;
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    // Otherwise give the content a chance to consume it.
                    if crate::route_event_single(&mut self.content, ctx, event)
                        == EventResult::Handled
                    {
                        return EventResult::Handled;
                    }
                    // Falls to the modal barrier: swallow, arming a scrim dismiss
                    // when the press started outside the panel.
                    self.scrim_captured = true;
                    self.scrim_down_outside = !self.panel.contains(p.position);
                    ctx.capture_pointer();
                    EventResult::Handled
                }
                // A captured content child keeps receiving Move/Up/Cancel via the
                // route helper's active-path fast lane.
                _ => crate::route_event_single(&mut self.content, ctx, event),
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Dialog,
            |node| node.set_modal(),
            |ctx| {
                self.content.semantics_child(ctx);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorView, navigator};
    use crate::nav::transition::TransitionSpec;
    use crate::test_support::leaf_any;
    use forgekit_core::{
        KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent, RenderRoot, any as core_any,
    };
    use forgekit_text::TextContext;
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn build(view: &BottomSheetView<()>) -> BottomSheetWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Records filled rects, rounded rects, and filled paths (as bounding-box
    /// origin + a brush color).
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        paths: Vec<Color>,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_path(&mut self, _origin: Point, _path: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.paths.push(*color);
            }
        }
    }

    #[test]
    fn layout_anchors_panel_to_bottom_with_handle_strip() {
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(area));
        assert_eq!(size, area, "the sheet fills the whole area (scrim)");

        // Panel: full width, bottom-anchored, sized to content + handle strip.
        assert_eq!(w.panel.x0, 0.0);
        assert_eq!(w.panel.x1, area.width);
        assert_eq!(w.panel.y1, area.height, "panel bottom is the screen edge");
        assert_eq!(w.panel.height(), HANDLE_TOUCH_TARGET + 200.0);
        // The handle strip is the top 48dp of the panel.
        assert_eq!(w.handle_target.y0, w.panel.y0);
        assert_eq!(w.handle_target.height(), HANDLE_TOUCH_TARGET);
        // Content sits below the handle strip.
        assert_eq!(w.content.origin().y, w.panel.y0 + HANDLE_TOUCH_TARGET);
    }

    #[test]
    fn unthemed_paint_draws_scrim_panel_and_handle() {
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);

        // Scrim: full-area fill at 32% of the fallback scrim.
        assert_eq!(rec.rects[0].1, area);
        assert_eq!(rec.rects[0].2, with_alpha(SCRIM, SCRIM_ALPHA));
        // Panel: a filled top-rounded path in the fallback container color.
        assert_eq!(rec.paths, vec![CONTAINER]);
        // Handle: a 32×4 rounded indicator in the fallback handle color.
        let handle = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| *s == Size::new(HANDLE_WIDTH, HANDLE_HEIGHT))
            .expect("the drag handle is painted");
        assert_eq!(handle.3, HANDLE_COLOR);
    }

    #[test]
    fn themed_paint_resolves_r16_tokens() {
        let theme = Theme::m3_baseline();
        let scheme = theme.scheme();
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rects[0].2, with_alpha(scheme.scrim, SCRIM_ALPHA));
        assert_eq!(rec.paths, vec![scheme.surface_container_low]);
        let handle = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| *s == Size::new(HANDLE_WIDTH, HANDLE_HEIGHT))
            .expect("the drag handle is painted");
        assert_eq!(handle.3, scheme.on_surface_variant);
    }

    // --- Drag-to-dismiss (threshold-simple). ---

    #[derive(Default)]
    struct Flag {
        dismissed: u32,
    }

    fn build_flag(view: &BottomSheetView<Flag>) -> BottomSheetWidget {
        let mut counter = 0u64;
        View::<Flag>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn dispatch(w: &mut BottomSheetWidget, state: &mut Flag, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, event)
    }

    fn laid_out_flag_sheet() -> BottomSheetWidget {
        let view: BottomSheetView<Flag> =
            bottom_sheet(leaf_any_flag(300.0, 200.0)).on_dismiss(|s: &mut Flag| s.dismissed += 1);
        let mut w = build_flag(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));
        w
    }

    // A `()`-erased leaf is `AnyView<()>`; the Flag-state sheet needs an
    // `AnyView<Flag>`, so wrap a fixed-size content view generic over state.
    fn leaf_any_flag(w: f64, h: f64) -> AnyView<Flag> {
        core_any::<Flag, _>(FixedLeaf {
            size: Size::new(w, h),
        })
    }
    struct FixedLeaf {
        size: Size,
    }
    struct FixedLeafW {
        size: Size,
    }
    impl View<Flag> for FixedLeaf {
        type Element = FixedLeafW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> FixedLeafW {
            FixedLeafW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut FixedLeafW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for FixedLeafW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
    }

    #[test]
    fn handle_drag_past_threshold_dismisses() {
        let mut w = laid_out_flag_sheet();
        // panel height = 48 + 200 = 248; threshold = 124.
        let handle_y = w.panel.y0 + 10.0; // inside the 48dp handle strip
        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        assert!(w.drag_active);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 200.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 200.0),
        );
        assert_eq!(state.dismissed, 1, "a drag past half the sheet height pops");
    }

    #[test]
    fn short_handle_drag_does_not_dismiss() {
        let mut w = laid_out_flag_sheet();
        let handle_y = w.panel.y0 + 10.0;
        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 20.0),
        );
        assert_eq!(state.dismissed, 0, "a short drag does not dismiss");
    }

    #[test]
    fn scrim_tap_outside_panel_dismisses() {
        let mut w = laid_out_flag_sheet();
        let mut state = Flag::default();
        // (5, 5) is in the top scrim, above the bottom-anchored panel.
        assert!(!w.panel.contains(Point::new(5.0, 5.0)));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
    }

    // --- Navigator integration: scrim tap pops with an empty result. ---

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<i32>>,
    }

    #[test]
    fn scrim_tap_pops_sheet_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // Push the sheet with an instant transition (no slide to settle),
        // wiring the dismiss to a plain pop.
        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // Tap the top scrim, above the bottom-anchored sheet.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "scrim tap pops with an empty result"
        );
    }

    // --- Focus + Escape opt-in (task 07, 6d). ---

    #[test]
    fn escape_after_a_short_handle_press_claims_focus_and_dismisses_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // panel height = 48 (handle strip) + 200 (content) = 248; panel_y = 352.
        // A short handle-strip drag (well under the 124px dismiss threshold)
        // claims focus without itself dismissing (mirrors
        // `short_handle_drag_does_not_dismiss`).
        let handle_y = area.height - 248.0 + 10.0;
        root.event(&mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        root.event(&mut state, &ev(PointerPhase::Up, 200.0, handle_y + 10.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        assert!(
            state.results.is_empty(),
            "a short handle drag does not dismiss"
        );

        // Escape now reaches the focused sheet and dismisses it, the same
        // dismiss path as a scrim tap / long handle drag.
        root.event(&mut state, &escape_event());
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "Escape dismisses the focused sheet"
        );
    }

    #[test]
    fn escape_without_a_prior_press_does_nothing() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // No press yet — the sheet never claimed focus, so Escape has no focus
        // chain to travel and is dropped.
        root.event(&mut state, &escape_event());
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert!(
            state.results.is_empty(),
            "Escape without prior focus is a no-op"
        );
    }

    // A minimal opaque page for the navigator integration test.
    struct BgPage {
        size: Size,
    }
    struct BgPageW {
        size: Size,
    }
    impl View<NavState> for BgPage {
        type Element = BgPageW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> BgPageW {
            BgPageW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut BgPageW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for BgPageW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }
    fn bg_page(w: f64, h: f64) -> BgPage {
        BgPage {
            size: Size::new(w, h),
        }
    }

    #[test]
    fn semantics_is_a_modal_dialog_ish_node() {
        fn logic(_s: &mut ()) -> BottomSheetView<()> {
            bottom_sheet(leaf_any(300.0, 200.0))
        }
        let mut root: RenderRoot<(), BottomSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal(), "the sheet node sets the modal flag");
    }
}
