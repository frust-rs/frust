//! M3 assist + filter `Chips` (Phase 6c, PLAN.md D5, task 07): 32dp height,
//! 8dp corner radius pill-shaped controls, using the shared
//! [`super::state_layer`] interaction overlay. Input/suggestion chip variants
//! are deferred (PLAN.md D5, out of v1 scope).
//!
//! [`assist_chip`] fires a plain `on_press` callback (an inert, non-toggling
//! action chip — its optional [`AssistChipView::leading`] glyph/icon is
//! rendered as a second nested [`crate::text`] run, not a dedicated `Icon`
//! widget, since the framework has none yet). [`filter_chip`] is a
//! **controlled component** mirroring [`crate::Checkbox`]/[`super::switch`]:
//! it reports the requested `selected` value through `on_select` and never
//! flips its own field — the app mutates its state and the next `rebuild`
//! feeds the confirmed value back in.
//!
//! # Label color simplification
//!
//! Both chip kinds paint their label through [`crate::text::ThemeTextColor::OnSurface`]
//! — the only themed-default label role `TextView` supports today besides
//! `OnPrimary` (button's role, wrong contrast against a chip's `surface`/
//! `secondary_container` fill). The M3 spec's exact selected-filter-chip
//! label role is `on_secondary_container`, tonally close to `on_surface` in
//! the M3 baseline palette (both dark-on-light) — a documented v1
//! approximation, not a fabricated value, pending a dedicated role added to
//! `TextView` in a later task.

use std::rc::Rc;

use forgekit_core::accesskit::{Action, Role, Toggled};
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use forgekit_theme::{ShapeScale, Theme};
use kurbo::{Point, Rect, Size};
use peniko::Color;

use super::state_layer::StateLayer;
use crate::text::{self, ThemeTextColor};

/// Fixed chip height, in logical px (M3 chip spec).
const CHIP_HEIGHT: f64 = 32.0;
/// Horizontal padding around the label when there is no leading glyph (and
/// always on the trailing/right side), in logical px.
const CHIP_PAD_X: f64 = 16.0;
/// Left padding before a leading glyph, in logical px (tighter than
/// [`CHIP_PAD_X`] since the glyph itself provides visual weight).
const LEADING_LEFT_PAD: f64 = 8.0;
/// Gap between a leading glyph and the label, in logical px.
const LEADING_GAP: f64 = 8.0;
/// Corner radius (unthemed fallback; a theme resolves this from
/// `shape.small`, the same 8dp token in the M3 baseline scale).
const CHIP_RADIUS: f64 = 8.0;

/// Assist-chip container fill (unthemed fallback; a theme resolves this from
/// `colors.surface_container_low`).
const ASSIST_CONTAINER: Color = Color::from_rgb8(0xF3, 0xF4, 0xF6);
/// Unselected filter-chip container fill — the same tone as
/// [`ASSIST_CONTAINER`] (unthemed fallback; a theme resolves this from
/// `colors.surface_container_low`).
const FILTER_UNSELECTED_CONTAINER: Color = Color::from_rgb8(0xF3, 0xF4, 0xF6);
/// Selected filter-chip container fill (unthemed fallback; a theme resolves
/// this from `colors.secondary_container` — this literal matches the M3
/// baseline `secondary_container` light-scheme tone exactly, see
/// `forgekit_theme::color::ColorScheme::m3_baseline_light`).
const FILTER_SELECTED_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Chip label color (unthemed fallback; a theme resolves this from
/// `colors.on_surface` — see the module docs' label-color simplification).
const CHIP_LABEL: Color = Color::from_rgb8(0x11, 0x18, 0x27);

/// The chip corner radius: themed `shape.small`, resolved against the box so
/// it never exceeds a pill. Unthemed: [`CHIP_RADIUS`] exactly.
fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.small, size.width, size.height),
        None => CHIP_RADIUS,
    }
}

/// The resolved `(container, label)` colors for an assist chip. Themed:
/// `surface_container_low`/`on_surface`. Unthemed: [`ASSIST_CONTAINER`]/
/// [`CHIP_LABEL`] exactly.
fn resolve_assist_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.surface_container_low, scheme.on_surface)
        }
        None => (ASSIST_CONTAINER, CHIP_LABEL),
    }
}

/// The resolved `(container, label)` colors for a filter chip at the given
/// `selected` state. Themed: `secondary_container`/`surface_container_low`
/// (per `selected`), label `on_surface` (see the module docs). Unthemed:
/// [`FILTER_SELECTED_CONTAINER`]/[`FILTER_UNSELECTED_CONTAINER`]/[`CHIP_LABEL`]
/// exactly.
fn resolve_filter_colors(theme: Option<&Theme>, selected: bool) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            let container = if selected {
                scheme.secondary_container
            } else {
                scheme.surface_container_low
            };
            (container, scheme.on_surface)
        }
        None => {
            let container = if selected {
                FILTER_SELECTED_CONTAINER
            } else {
                FILTER_UNSELECTED_CONTAINER
            };
            (container, CHIP_LABEL)
        }
    }
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

// ---------------------------------------------------------------------
// AssistChip
// ---------------------------------------------------------------------

/// Build the type-erased label view, themed `OnSurface` (see the module
/// docs). Shared by build/rebuild/teardown so the role stays consistent.
fn assist_label_view<State: 'static>(label: String) -> forgekit_core::AnyView<State> {
    any::<State, _>(text::text(label).themed_role(ThemeTextColor::OnSurface))
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative assist chip. See the [module docs](self).
pub struct AssistChipView<State: 'static> {
    label: String,
    leading: Option<String>,
    on_press: OnPress<State>,
}

/// Create an assist chip labelled `label` that runs `on_press` against the
/// app state when released inside its bounds.
pub fn assist_chip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> AssistChipView<State> {
    AssistChipView {
        label: label.into(),
        leading: None,
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`assist_chip`].
#[allow(non_snake_case)]
pub fn AssistChip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> AssistChipView<State> {
    assist_chip(label, on_press)
}

impl<State: 'static> AssistChipView<State> {
    /// Attach a leading glyph/icon (rendered as a second nested text run —
    /// see the module docs), painted before the label with a tighter left
    /// inset.
    pub fn leading(mut self, glyph: impl Into<String>) -> Self {
        self.leading = Some(glyph.into());
        self
    }
}

/// The retained widget for an [`AssistChipView`].
pub struct AssistChipWidget {
    leading: Option<ChildPod>,
    leading_text: Option<String>,
    label: ChildPod,
    label_text: String,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    state_layer: StateLayer,
    on_press: crate::ErasedCallback,
}

impl<State: 'static> View<State> for AssistChipView<State> {
    type Element = AssistChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AssistChipWidget {
        let label_view = assist_label_view::<State>(self.label.clone());
        let leading = self
            .leading
            .as_ref()
            .map(|glyph| crate::build_child(&assist_label_view::<State>(glyph.clone()), ctx));
        AssistChipWidget {
            leading,
            leading_text: self.leading.clone(),
            label: crate::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_press: crate::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AssistChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = crate::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = assist_label_view::<State>(prev.label.clone());
            let next_view = assist_label_view::<State>(self.label.clone());
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }

        match (&prev.leading, &self.leading) {
            (None, None) => {}
            (Some(prev_glyph), Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                let prev_view = assist_label_view::<State>(prev_glyph.clone());
                let next_view = assist_label_view::<State>(next_glyph.clone());
                let pod = element
                    .leading
                    .as_mut()
                    .expect("leading pod present when prev.leading is Some");
                flags |= crate::rebuild_child(&prev_view, &next_view, pod, ctx);
            }
            (None, Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                element.leading = Some(crate::build_child(
                    &assist_label_view::<State>(next_glyph.clone()),
                    ctx,
                ));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_glyph), None) => {
                element.leading_text = None;
                let prev_view = assist_label_view::<State>(prev_glyph.clone());
                let mut pod = element
                    .leading
                    .take()
                    .expect("leading pod present when prev.leading is Some");
                crate::teardown_child(&prev_view, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut AssistChipWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = assist_label_view::<State>(self.label.clone());
        crate::teardown_child(&label_view, &mut element.label, ctx);
        if let (Some(glyph), Some(pod)) = (&self.leading, &mut element.leading) {
            let leading_view = assist_label_view::<State>(glyph.clone());
            crate::teardown_child(&leading_view, pod, ctx);
        }
    }
}

impl Widget for AssistChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, CHIP_HEIGHT);
        let mut x = if self.leading.is_some() {
            LEADING_LEFT_PAD
        } else {
            CHIP_PAD_X
        };
        if let Some(leading) = &mut self.leading {
            let size = leading.layout_child(ctx, &BoxConstraints::loose(inner_max));
            leading.set_origin(Point::new(x, (CHIP_HEIGHT - size.height) / 2.0));
            x += size.width + LEADING_GAP;
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label
            .set_origin(Point::new(x, (CHIP_HEIGHT - label_size.height) / 2.0));
        x += label_size.width + CHIP_PAD_X;
        bc.constrain(Size::new(x, CHIP_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (container, label_color) = resolve_assist_colors(theme);
        let radius = resolve_radius(theme, ctx.size());
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, container);
        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_origin_size(ctx.origin(), ctx.size()),
            radius,
            label_color,
        );
        if let Some(leading) = &mut self.leading {
            leading.paint_child(ctx, scene);
        }
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
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
                    (self.on_press)(ctx);
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
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.add_action(Action::Click);
        });
    }
}

// ---------------------------------------------------------------------
// FilterChip
// ---------------------------------------------------------------------

/// Build the type-erased label view, themed `OnSurface` (see the module
/// docs). Shared by build/rebuild/teardown so the role stays consistent.
fn filter_label_view<State: 'static>(label: String) -> forgekit_core::AnyView<State> {
    any::<State, _>(text::text(label).themed_role(ThemeTextColor::OnSurface))
}

/// A view-held, typed select callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative filter chip. See the [module docs](self).
pub struct FilterChipView<State: 'static> {
    label: String,
    selected: bool,
    on_select: OnSelect<State>,
}

/// Create a filter chip labelled `label` reflecting `selected`, that fires
/// `on_select(state, !selected)` on release inside its bounds.
pub fn filter_chip<State: 'static, F: Fn(&mut State, bool) + 'static>(
    label: impl Into<String>,
    selected: bool,
    on_select: F,
) -> FilterChipView<State> {
    FilterChipView {
        label: label.into(),
        selected,
        on_select: Rc::new(on_select),
    }
}

/// PascalCase alias for [`filter_chip`].
#[allow(non_snake_case)]
pub fn FilterChip<State: 'static, F: Fn(&mut State, bool) + 'static>(
    label: impl Into<String>,
    selected: bool,
    on_select: F,
) -> FilterChipView<State> {
    filter_chip(label, selected, on_select)
}

/// The retained widget for a [`FilterChipView`].
pub struct FilterChipWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    selected: bool,
    label: ChildPod,
    label_text: String,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    state_layer: StateLayer,
    on_select: crate::ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for FilterChipView<State> {
    type Element = FilterChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FilterChipWidget {
        let label_view = filter_label_view::<State>(self.label.clone());
        FilterChipWidget {
            selected: self.selected,
            label: crate::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_select: crate::erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FilterChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = crate::erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;
        if prev.selected != self.selected {
            // The app is the source of truth: adopt the new value on rebuild.
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = filter_label_view::<State>(prev.label.clone());
            let next_view = filter_label_view::<State>(self.label.clone());
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        flags
    }

    fn teardown(&self, element: &mut FilterChipWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = filter_label_view::<State>(self.label.clone());
        crate::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for FilterChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, CHIP_HEIGHT);
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label.set_origin(Point::new(
            CHIP_PAD_X,
            (CHIP_HEIGHT - label_size.height) / 2.0,
        ));
        bc.constrain(Size::new(label_size.width + CHIP_PAD_X * 2.0, CHIP_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (container, label_color) = resolve_filter_colors(theme, self.selected);
        let radius = resolve_radius(theme, ctx.size());
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, container);
        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_origin_size(ctx.origin(), ctx.size()),
            radius,
            label_color,
        );
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
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
                    // Report the *requested* value; never self-toggle.
                    let requested = !self.selected;
                    (self.on_select)(ctx, requested);
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
        // No dedicated "toggle button"/"selectable chip" accesskit role
        // exists; a filter chip is exposed as a Button carrying Toggled
        // state (the common aria-pressed-style pattern for a toggleable
        // button-shaped control).
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.set_toggled(Toggled::from(self.selected));
            node.add_action(Action::Click);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(forgekit_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: forgekit_core::PointerButton::Primary,
        })
    }

    /// Records each rounded rect's `(origin, size, radius, color)`.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    fn paint_rec(w: &mut dyn Widget, size: Size, theme: Option<&Theme>) -> RRectRecorder {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // --- AssistChip ---

    mod assist {
        use super::*;

        #[derive(Default)]
        struct Presses(u32);

        fn widget() -> AssistChipWidget {
            let view = assist_chip::<Presses, _>("Go", |s: &mut Presses| s.0 += 1);
            let mut counter = 0u64;
            View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        fn dispatch(w: &mut AssistChipWidget, state: &mut Presses, event: &InputEvent) {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            w.event(&mut ctx, event);
        }

        #[test]
        fn up_inside_fires_once() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.0, 1);
        }

        #[test]
        fn up_outside_does_not_fire() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
            assert_eq!(state.0, 0);
        }

        #[test]
        fn cancel_clears_armed_state_without_firing() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            assert!(w.captured);
            dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 12.0));
            assert!(!w.captured);
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.0, 0);
        }

        #[test]
        fn hover_move_without_down_is_ignored_noop() {
            let mut w = widget();
            let mut state = Presses::default();
            let state_any: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            let result = w.event(&mut ctx, &ev(PointerPhase::Move, 5.0, 12.0));
            assert!(matches!(result, EventResult::Ignored));
            assert!(!ctx.needs_redraw());
            assert_eq!(state.0, 0);
        }

        #[test]
        fn unthemed_paint_uses_fallback_constants() {
            let mut w = widget();
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(
                rec.rrects[0],
                (
                    Point::ZERO,
                    Size::new(100.0, CHIP_HEIGHT),
                    CHIP_RADIUS,
                    ASSIST_CONTAINER
                )
            );
        }

        #[test]
        fn themed_paint_resolves_surface_container_low() {
            let theme = Theme::m3_baseline();
            let mut w = widget();
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_low);
            assert_eq!(rec.rrects[0].2, theme.shape.small);
        }

        #[test]
        fn leading_glyph_is_optional_and_shifts_label_layout() {
            use forgekit_text::TextContext;
            use std::any::Any;

            let mut counter = 0u64;
            let plain = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {});
            let mut w_plain = View::<Presses>::build(&plain, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size_plain =
                w_plain.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)));

            let with_leading = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {}).leading("*");
            let mut w_leading =
                View::<Presses>::build(&with_leading, &mut BuildCtx::new(&mut counter));
            let mut lctx2 = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size_leading =
                w_leading.layout(&mut lctx2, &BoxConstraints::loose(Size::new(400.0, 100.0)));

            assert!(
                size_leading.width > size_plain.width,
                "a leading glyph widens the chip"
            );
            assert_eq!(size_plain.height, CHIP_HEIGHT);
            assert_eq!(size_leading.height, CHIP_HEIGHT);
        }

        #[test]
        fn semantics_reports_button_role_and_label() {
            use forgekit_text::TextContext;
            use std::any::Any;

            fn logic(_s: &mut Presses) -> AssistChipView<Presses> {
                assist_chip::<Presses, _>("Go", |_s: &mut Presses| {})
            }
            let mut root: forgekit_core::RenderRoot<Presses, AssistChipView<Presses>> =
                forgekit_core::RenderRoot::new();
            let mut state = Presses::default();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Button)
                .expect("assist chip contributes a Role::Button node");
            assert_eq!(node.label(), Some("Go"));
            assert!(node.supports_action(Action::Click));
        }
    }

    // --- FilterChip ---

    mod filter {
        use super::*;

        #[derive(Default)]
        struct SelectState {
            last: Option<bool>,
            selects: u32,
        }

        fn widget(selected: bool) -> FilterChipWidget {
            let view =
                filter_chip::<SelectState, _>("Vegan", selected, |s: &mut SelectState, v: bool| {
                    s.last = Some(v);
                    s.selects += 1;
                });
            let mut counter = 0u64;
            View::<SelectState>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        fn dispatch(w: &mut FilterChipWidget, state: &mut SelectState, event: &InputEvent) {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            w.event(&mut ctx, event);
        }

        #[test]
        fn unselected_fires_true_and_does_not_self_mutate() {
            let mut w = widget(false);
            let mut state = SelectState::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.last, Some(true));
            assert_eq!(state.selects, 1);
            assert!(!w.selected, "chip must not mutate its own selected flag");
        }

        #[test]
        fn selected_fires_false() {
            let mut w = widget(true);
            let mut state = SelectState::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.last, Some(false));
            assert!(w.selected, "still selected until the app rebuilds it");
        }

        #[test]
        fn up_outside_does_not_fire() {
            let mut w = widget(false);
            let mut state = SelectState::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
            assert_eq!(state.selects, 0);
        }

        #[test]
        fn rebuild_adopts_new_selected_value_without_self_mutation() {
            let mut counter = 0u64;
            let prev = filter_chip::<SelectState, _>("Vegan", false, |_s, _v| {});
            let mut w = View::<SelectState>::build(&prev, &mut BuildCtx::new(&mut counter));
            assert!(!w.selected);
            let next = filter_chip::<SelectState, _>("Vegan", true, |_s, _v| {});
            let flags = View::<SelectState>::rebuild(
                &next,
                &prev,
                &mut w,
                &mut BuildCtx::new(&mut counter),
            );
            assert!(w.selected);
            assert!(flags.needs_paint());
        }

        #[test]
        fn unthemed_paint_uses_fallback_constants() {
            let mut off = widget(false);
            let rec = paint_rec(&mut off, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(rec.rrects[0].3, FILTER_UNSELECTED_CONTAINER);

            let mut on = widget(true);
            let rec = paint_rec(&mut on, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(rec.rrects[0].3, FILTER_SELECTED_CONTAINER);
        }

        #[test]
        fn themed_paint_resolves_secondary_container_when_selected() {
            let theme = Theme::m3_baseline();
            let mut off = widget(false);
            let rec = paint_rec(&mut off, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_low);

            let mut on = widget(true);
            let rec = paint_rec(&mut on, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert_eq!(rec.rrects[0].3, theme.scheme().secondary_container);
        }

        #[test]
        fn semantics_reports_toggled_state() {
            use forgekit_text::TextContext;
            use std::any::Any;

            fn logic(_s: &mut SelectState) -> FilterChipView<SelectState> {
                filter_chip::<SelectState, _>("Vegan", true, |_s, _v| {})
            }
            let mut root: forgekit_core::RenderRoot<SelectState, FilterChipView<SelectState>> =
                forgekit_core::RenderRoot::new();
            let mut state = SelectState::default();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Button)
                .expect("filter chip contributes a Role::Button node");
            assert_eq!(node.label(), Some("Vegan"));
            assert_eq!(node.toggled(), Some(Toggled::True));
        }
    }
}
