//! The M3 `Card` container (Phase 6c, PLAN.md D5, task 08): elevated /
//! filled / outlined variants, wrapping a single [`forgekit_core::AnyView`]
//! child (the same single-child `ChildPod` wrapper shape as [`crate::Padding`]).
//!
//! # Variants (ledger R10/R11)
//!
//! All three variants share the medium shape (12dp, `shape.medium`) and
//! `CARD_PADDING` (16dp) content inset — the standard M3 card spec
//! (m3.material.io/components/cards/specs). Per ledger R11:
//!
//! * **Elevated**: container `surfaceContainerLow`, M3 elevation level 1
//!   (1dp), painted via [`forgekit_core::PaintScene::draw_shadow`].
//! * **Filled**: container `surfaceContainerHighest`, elevation 0 (no
//!   shadow).
//! * **Outlined**: container `surface`, a 1dp stroke in `outlineVariant`
//!   (the enabled-state stroke color — `outline` is reserved for a disabled
//!   card, out of v1 scope since no card here models a disabled state), no
//!   shadow.
//!
//! # Outlined stroke primitive choice
//!
//! No stroked-rounded-rect primitive exists on [`forgekit_core::PaintScene`]
//! (only `stroke_line` and the task-05 `stroke_path`/`fill_path` pair). Rather
//! than approximate the rounded stroke with four `stroke_line` calls (visibly
//! square corners), this module builds a real rounded-rect outline via
//! `kurbo::RoundedRect` (a [`kurbo::Shape`], so `.to_path(tolerance)` yields a
//! `BezPath`) and paints it with [`forgekit_core::PaintScene::stroke_path`] —
//! the precedent this task sets for any future widget needing a stroked
//! rounded shape.
//!
//! # Interactivity
//!
//! [`CardView::on_press`] makes the whole card surface one interactive
//! target — mirroring [`crate::Button`] (fire-on-up-inside; the card itself
//! owns capture and paints the shared [`super::state_layer`] overlay, tinted
//! `on_surface`), rather than forwarding events into the child. Without
//! `on_press` the card is a transparent, non-interactive
//! wrapper that routes pointer events straight to its child (mirroring
//! [`crate::Padding`]).
//!
//! # Semantics
//!
//! An interactive card contributes a [`Role::Button`] container node (its
//! child's own semantics become the button's accesskit children — a card has
//! no single-line text label of its own to flatten into the node, unlike
//! `Button`). A non-interactive card contributes a [`Role::GenericContainer`]
//! ("group") node instead of transparently forwarding like `Padding` — the
//! task's explicit choice to keep a card's content grouped as one semantic
//! unit even when it isn't clickable.

use std::rc::Rc;

use forgekit_core::accesskit::{Action, Role};
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::state_layer::StateLayer;

/// Content padding on all four edges, in logical px (M3 card spec).
const CARD_PADDING: f64 = 16.0;
/// Corner radius (unthemed fallback; a theme resolves this from
/// `shape.medium`, ledger R12's 12dp token).
const RADIUS: f64 = 12.0;
/// Outline stroke width, in logical px (M3 card spec: hairline 1dp).
const STROKE_WIDTH: f64 = 1.0;
/// Flattening tolerance for the outlined variant's rounded-rect stroke path
/// (see the module docs' primitive-choice note) — a visually-lossless value
/// for on-screen corner radii.
const STROKE_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback elevated-card container fill (a theme resolves this from
/// `colors.surface_container_low`).
const ELEVATED_CONTAINER: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback filled-card container fill (a theme resolves this from
/// `colors.surface_container_highest`).
const FILLED_CONTAINER: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback outlined-card container fill (a theme resolves this from
/// `colors.surface`).
const OUTLINED_CONTAINER: Color = Color::from_rgb8(0xFE, 0xF7, 0xFF);
/// Unthemed-fallback outline stroke color (a theme resolves this from
/// `colors.outline_variant` — the enabled-state stroke, ledger R11).
const OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);
/// Unthemed-fallback state-layer content color for an interactive card (a
/// theme resolves this from `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// Unthemed-fallback shadow y-offset, matching `Elevation::m3().level1`'s
/// `y_offset` exactly (`dp / 2.0 + 1.0` at `dp = 1.0`).
const FALLBACK_SHADOW_Y_OFFSET: f64 = 1.5;
/// Unthemed-fallback shadow blur std-dev, matching `Elevation::m3().level1`.
const FALLBACK_SHADOW_BLUR: f64 = 1.0;
/// Unthemed-fallback shadow color (opaque black at `Elevation::m3().level1`'s
/// `0.3` alpha).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The M3 card container variant (ledger R10/R11). See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardVariant {
    Elevated,
    Filled,
    Outlined,
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved container fill for `variant`. Themed: `surface_container_low`
/// (elevated) / `surface_container_highest` (filled) / `surface` (outlined).
/// Unthemed: [`ELEVATED_CONTAINER`]/[`FILLED_CONTAINER`]/[`OUTLINED_CONTAINER`]
/// exactly.
fn resolve_container(theme: Option<&Theme>, variant: CardVariant) -> Color {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            match variant {
                CardVariant::Elevated => scheme.surface_container_low,
                CardVariant::Filled => scheme.surface_container_highest,
                CardVariant::Outlined => scheme.surface,
            }
        }
        None => match variant {
            CardVariant::Elevated => ELEVATED_CONTAINER,
            CardVariant::Filled => FILLED_CONTAINER,
            CardVariant::Outlined => OUTLINED_CONTAINER,
        },
    }
}

/// The resolved corner radius. Themed: `shape.medium`. Unthemed: [`RADIUS`]
/// exactly.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.medium,
        None => RADIUS,
    }
}

/// The resolved outline stroke color (outlined variant only, enabled state —
/// see the module docs). Themed: `colors.outline_variant`. Unthemed:
/// [`OUTLINE_VARIANT`] exactly.
fn resolve_outline(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().outline_variant,
        None => OUTLINE_VARIANT,
    }
}

/// The resolved state-layer content color for an interactive card. Themed:
/// `colors.on_surface`. Unthemed: [`ON_SURFACE`] exactly.
fn resolve_content_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters at M3
/// elevation level 1 (the elevated variant's resting elevation, ledger R11).
/// Themed: `theme.elevation.level1`'s `ShadowSpec`, colored by `colors.shadow`
/// at the spec's `color_alpha`. Unthemed: the [`FALLBACK_SHADOW_BLUR`]/
/// [`FALLBACK_SHADOW_Y_OFFSET`]/[`FALLBACK_SHADOW_COLOR`] constants exactly.
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = theme.elevation.level1;
            let color = with_alpha(theme.scheme().shadow, level.shadow.color_alpha);
            (level.shadow.blur_std_dev, level.shadow.y_offset, color)
        }
        None => (
            FALLBACK_SHADOW_BLUR,
            FALLBACK_SHADOW_Y_OFFSET,
            FALLBACK_SHADOW_COLOR,
        ),
    }
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 card wrapping a single child. See the [module docs](self).
pub struct CardView<State: 'static> {
    variant: CardVariant,
    child: AnyView<State>,
    on_press: Option<OnPress<State>>,
}

/// Wrap `child` in a card of the given `variant`. Chain [`CardView::on_press`]
/// to make the whole surface interactive.
pub fn card<State: 'static, V: View<State>>(variant: CardVariant, child: V) -> CardView<State> {
    CardView {
        variant,
        child: any(child),
        on_press: None,
    }
}

/// Wrap `child` in an elevated card (`surfaceContainerLow`, M3 elevation
/// level 1).
pub fn elevated_card<State: 'static, V: View<State>>(child: V) -> CardView<State> {
    card(CardVariant::Elevated, child)
}

/// Wrap `child` in a filled card (`surfaceContainerHighest`, no elevation).
pub fn filled_card<State: 'static, V: View<State>>(child: V) -> CardView<State> {
    card(CardVariant::Filled, child)
}

/// Wrap `child` in an outlined card (`surface` fill, `outlineVariant` stroke).
pub fn outlined_card<State: 'static, V: View<State>>(child: V) -> CardView<State> {
    card(CardVariant::Outlined, child)
}

impl<State: 'static> CardView<State> {
    /// Make the whole card surface interactive, firing `on_press` on release
    /// inside its bounds (see the module docs' Interactivity section).
    pub fn on_press<F: Fn(&mut State) + 'static>(mut self, on_press: F) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }
}

/// The retained widget for a [`CardView`].
pub struct CardWidget {
    variant: CardVariant,
    child: ChildPod,
    interactive: bool,
    pressed: bool,
    captured: bool,
    state_layer: StateLayer,
    on_press: Option<crate::ErasedCallback>,
}

impl<State: 'static> View<State> for CardView<State> {
    type Element = CardWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CardWidget {
        CardWidget {
            variant: self.variant,
            child: crate::build_child(&self.child, ctx),
            interactive: self.on_press.is_some(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_press: self.on_press.as_ref().map(crate::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CardWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        let now_interactive = self.on_press.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            // Losing the interactive surface mid-gesture must not leave a
            // dangling capture behind.
            if !now_interactive && element.captured {
                element.pressed = false;
                element.captured = false;
                element.state_layer.set_pressed(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        element.on_press = self.on_press.as_ref().map(crate::erase_callback);
        flags |= crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut CardWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for CardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inset = CARD_PADDING * 2.0;
        let inner_max = Size::new(
            (bc.max().width - inset).max(0.0),
            (bc.max().height - inset).max(0.0),
        );
        let inner_min = Size::new(
            (bc.min().width - inset).max(0.0),
            (bc.min().height - inset).max(0.0),
        );
        let child_size = self
            .child
            .layout_child(ctx, &BoxConstraints::new(inner_min, inner_max));
        self.child
            .set_origin(Point::new(CARD_PADDING, CARD_PADDING));
        bc.constrain(Size::new(
            child_size.width + inset,
            child_size.height + inset,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let container = resolve_container(theme, self.variant);
        let radius = resolve_radius(theme);
        let o = ctx.origin();
        let size = ctx.size();

        if self.variant == CardVariant::Elevated {
            let (blur, y_offset, shadow_color) = resolve_shadow(theme);
            scene.draw_shadow(
                Point::new(o.x, o.y + y_offset),
                size,
                radius,
                blur,
                shadow_color,
            );
        }

        scene.fill_rounded_rect(o, size, radius, container);

        if self.variant == CardVariant::Outlined {
            let outline = resolve_outline(theme);
            // Inset by half the stroke width so the 1dp line paints fully
            // inside the card's own bounds (a stroke is centered on its path).
            let half = STROKE_WIDTH / 2.0;
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (radius - half).max(0.0),
            );
            let path = rr.to_path(STROKE_TOLERANCE);
            scene.stroke_path(o, &path, STROKE_WIDTH, &Brush::Solid(outline));
        }

        if self.interactive {
            let content_color = resolve_content_color(theme);
            self.state_layer.paint(
                ctx,
                scene,
                Rect::from_origin_size(o, size),
                radius,
                content_color,
            );
        }

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
            return crate::route_event_single(&mut self.child, ctx, event);
        }
        // Non-pointer events (Key, Ime, focus-routed) must be forwarded to
        // the child, even when interactive. Only pointer events drive the
        // interactive card's own capture/press behavior.
        let InputEvent::Pointer(p) = event else {
            return crate::route_event_single(&mut self.child, ctx, event);
        };
        let on_press = self
            .on_press
            .as_mut()
            .expect("on_press is set whenever interactive is true");
        match p.phase {
            PointerPhase::Down => {
                self.pressed = true;
                self.captured = true;
                self.state_layer.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                self.pressed = inside_now;
                self.state_layer.set_pressed(inside_now);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.interactive {
            ctx.push_container(
                Role::Button,
                |node| {
                    node.add_action(Action::Click);
                },
                |ctx| {
                    self.child.semantics_child(ctx);
                },
            );
        } else {
            ctx.push_container(
                Role::GenericContainer,
                |_| {},
                |ctx| {
                    self.child.semantics_child(ctx);
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf_any;
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(forgekit_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: forgekit_core::PointerButton::Primary,
        })
    }

    /// Records each rounded rect's `(origin, size, radius, color)`, each
    /// shadow's `(origin, size, radius, std_dev, color)`, and each stroked
    /// path's `(origin, width, color)`.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        strokes: Vec<(Point, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn stroke_path(
            &mut self,
            origin: Point,
            _path: &kurbo::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(color) = brush {
                self.strokes.push((origin, width, *color));
            }
        }
    }

    fn build<S: 'static>(view: &CardView<S>) -> CardWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn paint(w: &mut CardWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn layout_insets_child_by_card_padding() {
        let view: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0 + 32.0, 20.0 + 32.0));
        assert_eq!(w.child.origin(), Point::new(CARD_PADDING, CARD_PADDING));
    }

    #[test]
    fn elevated_unthemed_paints_container_and_shadow() {
        let view: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.rrects[0].3, ELEVATED_CONTAINER);
        assert_eq!(rec.rrects[0].2, RADIUS);
        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].3, FALLBACK_SHADOW_BLUR);
        assert!(rec.strokes.is_empty(), "elevated card has no stroke");
    }

    #[test]
    fn filled_unthemed_paints_container_with_no_shadow() {
        let view: CardView<()> = filled_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.rrects[0].3, FILLED_CONTAINER);
        assert!(rec.shadows.is_empty(), "filled card paints no shadow");
        assert!(rec.strokes.is_empty(), "filled card has no stroke");
    }

    #[test]
    fn outlined_unthemed_paints_container_and_stroke_with_no_shadow() {
        let view: CardView<()> = outlined_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.rrects[0].3, OUTLINED_CONTAINER);
        assert!(rec.shadows.is_empty(), "outlined card paints no shadow");
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1, STROKE_WIDTH);
        assert_eq!(rec.strokes[0].2, OUTLINE_VARIANT);
    }

    #[test]
    fn themed_paint_resolves_r11_tokens() {
        let theme = Theme::m3_baseline();
        let scheme = theme.scheme();

        let elevated: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&elevated);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface_container_low);
        assert_eq!(rec.rrects[0].2, theme.shape.medium);

        let filled: CardView<()> = filled_card(leaf_any(40.0, 20.0));
        let mut w = build(&filled);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface_container_highest);

        let outlined: CardView<()> = outlined_card(leaf_any(40.0, 20.0));
        let mut w = build(&outlined);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface);
        assert_eq!(rec.strokes[0].2, scheme.outline_variant);
    }

    // --- Non-interactive: transparent event routing to the child ---

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn dispatch<S: 'static>(w: &mut CardWidget, state: &mut S, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(200.0, 200.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn non_interactive_card_forwards_events_to_its_child() {
        let view: CardView<Counter> =
            elevated_card(crate::button::<Counter, _>("go", |s: &mut Counter| {
                s.presses += 1
            }));
        let mut w = build(&view);
        let mut tcx = forgekit_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        let origin = w.child.origin();
        let mut state = Counter::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, origin.x + 2.0, origin.y + 2.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, origin.x + 2.0, origin.y + 2.0),
        );
        assert_eq!(state.presses, 1, "the nested button fires through the card");
    }

    // --- Interactive: whole-surface capture ---

    /// A minimal `State`-generic content stand-in (`test_support::leaf_any`
    /// only produces an `AnyView<()>`; the interactive tests below need a
    /// generic `State`, so a plain [`crate::text::text`] run stands in for
    /// the child content — irrelevant to firing behavior).
    fn content_stub<S: 'static>() -> AnyView<S> {
        any::<S, _>(crate::text::text("content"))
    }

    #[test]
    fn interactive_card_fires_on_up_inside_without_forwarding_to_child() {
        let view: CardView<Counter> =
            filled_card(content_stub::<Counter>()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn interactive_card_up_outside_does_not_fire() {
        let view: CardView<Counter> =
            filled_card(content_stub::<Counter>()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 500.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 500.0));
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn interactive_card_cancel_clears_armed_state() {
        let view: CardView<Counter> =
            filled_card(content_stub::<Counter>()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 5.0));
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 0);
    }

    // --- Interactive with focus-routed child events ---

    /// A minimal widget that consumes focus and handles Key events for testing.
    struct FocusConsumer;

    impl FocusConsumer {
        fn new() -> Self {
            FocusConsumer
        }
    }

    struct FocusConsumerWidget;

    impl View<Counter> for FocusConsumer {
        type Element = FocusConsumerWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FocusConsumerWidget {
            FocusConsumerWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FocusConsumerWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> forgekit_core::ChangeFlags {
            forgekit_core::ChangeFlags::NONE
        }
    }

    impl Widget for FocusConsumerWidget {
        fn layout(&mut self, _ctx: &mut forgekit_core::LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            // Handle any key event (focus-routed events reach here only if the
            // pod is focused, so a Key event here proves the routing worked).
            if let InputEvent::Key(_) = event {
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn interactive_card_forwards_key_events_to_focused_child() {
        let view: CardView<Counter> =
            filled_card(FocusConsumer::new()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();

        // Manually set the child as focused to simulate a prior focus state.
        // (In real usage, the child would be focused by a pointer-down event,
        // but here we're directly testing the event-routing path.)
        w.child.set_focused(true);

        // Send a Key event — it should be forwarded to the focused child.
        let key_event = InputEvent::Key(forgekit_core::KeyEvent {
            key: forgekit_core::Key::Named(forgekit_core::NamedKey::Backspace),
            modifiers: forgekit_core::Modifiers::default(),
            repeat: false,
        });
        let result = dispatch(&mut w, &mut state, &key_event);

        // Verify the key event was handled (forwarded to and handled by child).
        assert_eq!(
            result,
            EventResult::Handled,
            "key event must be forwarded to child"
        );

        // The card press should not have fired (the card only fires on pointer Up).
        assert_eq!(
            state.presses, 0,
            "card press callback does not fire on key event"
        );
    }

    #[test]
    fn semantics_interactive_card_is_a_button_container_forwarding_child() {
        // A leaf without its own `semantics()` override (e.g. `test_support`'s
        // `Leaf`) contributes no node — use a `Text` child, which does, so the
        // "forwards the child's semantics" assertion is meaningful.
        fn logic(_s: &mut ()) -> CardView<()> {
            filled_card(crate::text::text("body")).on_press(|_s: &mut ()| {})
        }
        let mut root: forgekit_core::RenderRoot<(), CardView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = forgekit_text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("interactive card contributes a Role::Button container node");
        assert!(
            !node.children().is_empty(),
            "the child is a semantics child"
        );
    }

    #[test]
    fn semantics_non_interactive_card_is_a_generic_container() {
        fn logic(_s: &mut ()) -> CardView<()> {
            filled_card(crate::text::text("body"))
        }
        let mut root: forgekit_core::RenderRoot<(), CardView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = forgekit_text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::GenericContainer)
            .expect("non-interactive card contributes a Role::GenericContainer node");
        assert!(
            !node.children().is_empty(),
            "the child is a semantics child"
        );
    }
}
