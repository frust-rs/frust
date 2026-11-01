//! [`fab`]/[`FabView`]: a floating action button — a circular accent-filled
//! control carrying one vector icon, lifted off the page by the `floating`
//! elevation token ([`crate::tokens::scales::floating`]).
//!
//! The shape follows Flutter's `FloatingActionButton` and mirrors
//! `frust-material`'s `fab(icon, on_press)` signature: a required icon and
//! press callback, with [`FabView::mini`] for the compact size,
//! [`FabView::label`] as the accessible name (Flutter's `tooltip`; strongly
//! recommended, since an icon alone says nothing to a screen reader) and
//! [`FabView::disabled`]. It composes with `frust::scaffold`'s fab slot, which
//! accepts any view and owns placement and margin.
//!
//! # Sizes
//!
//! Regular **46 px**, mini **40 px** (the Glyph design's sizes), with a 24 px
//! icon centred in the circle.
//!
//! # Color resolution
//!
//! Fill is the theme's `primary`, the icon ink `on_primary`, the shadow the
//! `shadow` role at the `floating` level's per-brightness alpha. Unthemed, each
//! falls back to the literal Glyph **dark** constant.
//!
//! # States
//!
//! - **Pressed** (primary button, captured while held): the fill takes a
//!   `on_primary` tint. The change is instant, so there is no animation to
//!   pace and nothing to special-case under `reduce_motion`: the widget never
//!   requests a frame.
//! - **Disabled**: the shadow is dropped and fill/ink dim to `on_surface` at
//!   `.12`/`.38`; the control ignores pointer input and reports disabled
//!   semantics instead of a Click action.
//!
//! A press fires on pointer-up inside the circle; releasing outside, or a
//! cancel, fires nothing (see `docs/CODE_STANDARDS.md`'s Interaction
//! Semantics).

use std::rc::Rc;

use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, View, Widget,
};
use frust::{IconData, Theme};
use kurbo::{Affine, BezPath, Point, Size};
use peniko::{Brush, Color};

use crate::press::presses;

/// Regular diameter, in logical px.
const FAB_SIZE: f64 = 46.0;
/// Mini diameter, in logical px.
const FAB_MINI_SIZE: f64 = 40.0;
/// Painted icon side, in logical px (the design's 24 px glyph).
const ICON_SIZE: f64 = 24.0;
/// Opacity of the `on_primary` tint laid over the fill while pressed.
const PRESSED_TINT: f32 = 0.14;
/// Disabled fill opacity (`on_surface`).
const DISABLED_FILL_ALPHA: f32 = 0.12;
/// Disabled icon opacity (`on_surface`).
const DISABLED_INK_ALPHA: f32 = 0.38;

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

const FALLBACK_FILL: Color = Color::from_rgb8(0xff, 0xb6, 0x27); // amber accent
const FALLBACK_INK: Color = Color::from_rgb8(0x24, 0x1a, 0x04); // on-amber
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // fg
const FALLBACK_SHADOW_Y: f64 = 6.0;
const FALLBACK_SHADOW_BLUR: f64 = 16.0;
const FALLBACK_SHADOW_ALPHA: f32 = 0.35;

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative floating action button. See the [module docs](self).
pub struct FabView<State: 'static> {
    icon: IconData,
    on_press: OnPress<State>,
    mini: bool,
    label: Option<String>,
    disabled: bool,
}

/// Create a floating action button showing `icon` that runs `on_press`
/// against the app state when released inside it.
pub fn fab<State: 'static, F: Fn(&mut State) + 'static>(
    icon: impl Into<IconData>,
    on_press: F,
) -> FabView<State> {
    FabView {
        icon: icon.into(),
        on_press: Rc::new(on_press),
        mini: false,
        label: None,
        disabled: false,
    }
}

impl<State: 'static> FabView<State> {
    /// Use the compact 40 px size instead of the default 46 px.
    pub fn mini(mut self, mini: bool) -> Self {
        self.mini = mini;
        self
    }

    /// Set the accessible name (Flutter's `tooltip`). Recommended: the icon
    /// alone carries no name for assistive technology.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Disable the button: dimmed, no shadow, no press, no action.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<State: 'static> View<State> for FabView<State> {
    type Element = FabWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FabWidget {
        let (path, design) = self.icon.resolve();
        FabWidget {
            icon: self.icon.clone(),
            path,
            design,
            mini: self.mini,
            label: self.label.clone(),
            disabled: self.disabled,
            pressed: false,
            captured: false,
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FabWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        element.label = self.label.clone();
        let mut flags = ChangeFlags::NONE;
        if !prev.icon.same(&self.icon) {
            let (path, design) = self.icon.resolve();
            element.icon = self.icon.clone();
            element.path = path;
            element.design = design;
            flags |= ChangeFlags::PAINT;
        }
        if element.mini != self.mini {
            element.mini = self.mini;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.pressed = false;
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`FabView`].
pub struct FabWidget {
    /// Retained for `rebuild`'s cheap same-icon check.
    icon: IconData,
    /// The parsed icon path in its design-box space.
    path: BezPath,
    design: f64,
    mini: bool,
    label: Option<String>,
    disabled: bool,
    pressed: bool,
    captured: bool,
    on_press: frust::authoring::ErasedCallback,
}

impl FabWidget {
    fn diameter(&self) -> f64 {
        if self.mini { FAB_MINI_SIZE } else { FAB_SIZE }
    }

    /// Whether `pos` (widget-local) is inside the circle.
    fn hit(&self, pos: Point) -> bool {
        let r = self.diameter() / 2.0;
        let (dx, dy) = (pos.x - r, pos.y - r);
        dx * dx + dy * dy <= r * r
    }

    /// `(fill, ink)` for the current state.
    fn resolve_colors(&self, theme: Option<&Theme>) -> (Color, Color) {
        if self.disabled {
            let base = theme.map_or(FALLBACK_ON_SURFACE, |t| t.scheme().on_surface);
            return (
                with_alpha(base, DISABLED_FILL_ALPHA),
                with_alpha(base, DISABLED_INK_ALPHA),
            );
        }
        let (fill, ink) = match theme {
            Some(t) => (t.scheme().primary, t.scheme().on_primary),
            None => (FALLBACK_FILL, FALLBACK_INK),
        };
        if self.pressed {
            (mix(fill, ink, PRESSED_TINT), ink)
        } else {
            (fill, ink)
        }
    }

    fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
        match theme {
            Some(t) => {
                let shadow = crate::tokens::scales::floating();
                let spec = shadow.shadow(t.brightness);
                (
                    spec.y_offset,
                    spec.blur_std_dev,
                    with_alpha(t.scheme().shadow, spec.color_alpha),
                )
            }
            None => (
                FALLBACK_SHADOW_Y,
                FALLBACK_SHADOW_BLUR,
                with_alpha(Color::from_rgb8(0, 0, 0), FALLBACK_SHADOW_ALPHA),
            ),
        }
    }
}

impl Widget for FabWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.diameter(), self.diameter()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (fill, ink) = self.resolve_colors(theme);
        let origin = ctx.origin();
        let size = ctx.size();
        let radius = size.width.min(size.height) / 2.0;

        if !self.disabled {
            let (y, blur, color) = Self::resolve_shadow(theme);
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y),
                size,
                radius,
                blur,
                color,
            );
        }
        scene.fill_rounded_rect(origin, size, radius, fill);

        let scale = if self.design > 0.0 {
            ICON_SIZE / self.design
        } else {
            1.0
        };
        let dx = (size.width - ICON_SIZE) / 2.0;
        let dy = (size.height - ICON_SIZE) / 2.0;
        let glyph = Affine::translate((dx, dy)) * Affine::scale(scale) * self.path.clone();
        scene.fill_path(origin, &glyph, &Brush::Solid(ink));
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.disabled {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !self.hit(p.position) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside = self.hit(p.position);
                if inside != self.pressed {
                    self.pressed = inside;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let fired = self.hit(p.position);
                self.pressed = false;
                self.captured = false;
                if fired {
                    (self.on_press)(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

/// `color` with its alpha replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// `over` blended onto `base` at opacity `t` (alpha of `base` kept).
fn mix(base: Color, over: Color, t: f32) -> Color {
    let (b, o) = (base.components, over.components);
    Color::new([
        b[0] + (o[0] - b[0]) * t,
        b[1] + (o[1] - b[1]) * t,
        b[2] + (o[2] - b[2]) * t,
        b[3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use frust::authoring::text::TextContext;
    use kurbo::Rect;

    #[derive(Default)]
    struct Taps {
        count: u32,
    }

    #[derive(Debug, PartialEq)]
    enum Op {
        Shadow(Size),
        Circle(Point, Size, f64, Color),
        Icon(Color),
    }

    #[derive(Default)]
    struct Recorder {
        ops: Vec<Op>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.ops.push(Op::Circle(o, s, radius, color));
        }
        fn draw_shadow(&mut self, _o: Point, s: Size, _r: f64, _std: f64, _c: Color) {
            self.ops.push(Op::Shadow(s));
        }
        fn fill_path(&mut self, _o: Point, _p: &BezPath, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.ops.push(Op::Icon(*c));
            }
        }
    }

    fn icon_data() -> IconData {
        IconData::from_path(
            kurbo::Shape::to_path(&Rect::new(4.0, 4.0, 20.0, 20.0), 0.1),
            24.0,
        )
    }

    fn view() -> FabView<Taps> {
        fab(icon_data(), |s: &mut Taps| s.count += 1).label("Add")
    }

    fn build(view: &FabView<Taps>) -> FabWidget {
        let mut counter = 0u64;
        View::<Taps>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut FabWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)))
    }

    fn paint(w: &mut FabWidget, size: Size, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut FabWidget, taps: &mut Taps, event: &InputEvent) {
        let any: &mut dyn Any = taps;
        let mut ctx = EventCtx::new(any, Point::ZERO, Size::new(46.0, 46.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn layout_is_46_default_and_40_mini() {
        let mut w = build(&view());
        assert_eq!(layout(&mut w), Size::new(46.0, 46.0));
        let mut m = build(&view().mini(true));
        assert_eq!(layout(&mut m), Size::new(40.0, 40.0));
    }

    #[test]
    fn paints_shadow_then_primary_circle_then_icon_within_bounds() {
        let theme = crate::baseline();
        let mut w = build(&view());
        let size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.ops.len(), 3);
        assert_eq!(rec.ops[0], Op::Shadow(size));
        assert_eq!(
            rec.ops[1],
            Op::Circle(Point::ZERO, size, 23.0, scheme.primary)
        );
        assert_eq!(rec.ops[2], Op::Icon(scheme.on_primary));
    }

    #[test]
    fn down_up_inside_fires_once() {
        let mut w = build(&view());
        layout(&mut w);
        let mut taps = Taps::default();
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 23.0, 23.0));
        assert!(w.captured && w.pressed);
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Up, 23.0, 23.0));
        assert_eq!(taps.count, 1);
        assert!(!w.captured && !w.pressed);
    }

    #[test]
    fn up_outside_or_cancel_fires_nothing() {
        let mut w = build(&view());
        layout(&mut w);
        let mut taps = Taps::default();
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 23.0, 23.0));
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Up, 200.0, 200.0));
        assert_eq!(taps.count, 0);
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 23.0, 23.0));
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Cancel, 23.0, 23.0));
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Up, 23.0, 23.0));
        assert_eq!(taps.count, 0);
    }

    #[test]
    fn corner_outside_the_circle_does_not_press() {
        let mut w = build(&view());
        layout(&mut w);
        let mut taps = Taps::default();
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 1.0, 1.0));
        assert!(!w.captured);
    }

    #[test]
    fn disabled_fires_nothing_and_paints_dimmed() {
        let theme = crate::baseline();
        let mut w = build(&view().disabled(true));
        let size = layout(&mut w);
        let mut taps = Taps::default();
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 23.0, 23.0));
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Up, 23.0, 23.0));
        assert_eq!(taps.count, 0);
        assert!(!w.captured);
        let (rec, _) = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        // No shadow: circle then icon, both dimmed off the accent.
        assert_eq!(rec.ops.len(), 2);
        let Op::Circle(_, _, _, fill) = rec.ops[0] else {
            panic!("expected circle first, got {:?}", rec.ops[0]);
        };
        assert_ne!(fill, scheme.primary);
        assert_eq!(fill, with_alpha(scheme.on_surface, DISABLED_FILL_ALPHA));
        assert_eq!(
            rec.ops[1],
            Op::Icon(with_alpha(scheme.on_surface, DISABLED_INK_ALPHA))
        );
    }

    #[test]
    fn pressed_tints_the_fill() {
        let theme = crate::baseline();
        let mut w = build(&view());
        let size = layout(&mut w);
        let mut taps = Taps::default();
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 23.0, 23.0));
        let (rec, _) = paint(&mut w, size, Some(&theme));
        let Op::Circle(_, _, _, fill) = rec.ops[1] else {
            panic!("expected circle");
        };
        assert_ne!(fill, theme.scheme().primary);
    }

    #[test]
    fn semantics_reports_button_label_and_click() {
        fn logic(_s: &mut Taps) -> FabView<Taps> {
            fab(icon_data(), |_s: &mut Taps| {}).label("Add")
        }
        let mut root: frust_core::RenderRoot<Taps, FabView<Taps>> = frust_core::RenderRoot::new();
        let mut state = Taps::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("fab contributes a Role::Button node");
        assert_eq!(node.label(), Some("Add"));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn reduce_motion_press_requests_no_frame() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build(&view());
        let size = layout(&mut w);
        let mut taps = Taps::default();
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Down, 23.0, 23.0));
        let (_, needs_frame) = paint(&mut w, size, Some(&theme));
        assert!(!needs_frame);
        dispatch(&mut w, &mut taps, &ev(PointerPhase::Up, 23.0, 23.0));
        let (_, needs_frame) = paint(&mut w, size, Some(&theme));
        assert!(!needs_frame);
    }
}
