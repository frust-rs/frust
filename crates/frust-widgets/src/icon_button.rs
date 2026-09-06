//! The `IconButton` interactive widget: a pressable **vector** icon — `mark`
//! centered in a square slot, labelled `label` for accessibility, firing
//! `on_press` on a release *inside* its bounds.
//!
//! [`icon_button`] is the declarative view-fn; it produces an
//! [`IconButtonView`] carrying the icon, the accessible label, and a typed
//! `on_press` closure, which materialises into a retained
//! [`IconButtonWidget`]. The interaction is the same **fire-on-up-inside**
//! contract every interactive widget in this crate follows (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics, and [`crate::button`]'s
//! own doc comment): a `Down` inside captures the pointer and paints the
//! pressed wash; `Move` only updates the pressed visual (cursor-inside); the
//! callback fires on `Up` *only if the release lands inside*; a `Cancel`
//! (platform gesture steal) clears the pressed state without firing.
//!
//! # Transparent at rest, pressed wash on press
//!
//! Unlike [`crate::button`]'s [`crate::ButtonStyle::Icon`] variant (a
//! square, filled/outlined affordance built for a labelled action row), this
//! widget paints **nothing** at rest — the mark is the whole resting
//! appearance, matching a toolbar/app-bar icon affordance (Material's
//! "standard icon button", the muxr topbar's trailing action slot). A `Down`
//! inside washes the slot in the mark's own resolved ink at
//! [`crate::authoring::PRESSED_OPACITY`], so a light-on-dark slot and a
//! dark-on-light one both darken correctly rather than one of them glowing
//! against its own background.
//!
//! # Slot and glyph sizing
//!
//! [`IconButtonView::icon_size`] sets the painted glyph's side length
//! (default [`DEFAULT_ICON_SIZE`], the same 24px default [`crate::icon`]
//! itself uses). [`IconButtonView::slot_size`] sets an explicit square tap
//! target; left unset, the slot is derived the same way
//! [`crate::ButtonStyle::Icon`] derives its own square box — pad the glyph by
//! [`PAD_X`]/[`PAD_Y`] on each axis and take the larger side — which lands a
//! default 24px glyph inside a 48×48 slot, matching Material's minimum
//! touch-target guidance (source:
//! <https://m3.material.io/foundations/designing/structure#touch-target>,
//! retrieved 2026-08-19) without hardcoding that number directly.
//!
//! # Color resolution
//!
//! The mark's ink follows [`crate::icon`]'s own **explicit builder value >
//! theme > fallback** precedence (see [`IconButtonView::ink`]): an explicit
//! override always wins; otherwise the default resolves `on_surface` from
//! the threaded [`Theme`], falling back to [`UNTHEMED_INK`] (matching
//! `icon.rs`'s own unthemed default) when no theme is threaded. The pressed
//! wash reuses this same resolved ink rather than a separate color, so it
//! never needs a resolution of its own.

use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

use crate::authoring::{ErasedCallback, PRESSED_OPACITY, erase_callback, presses};
use crate::icon;
use crate::icon::IconData;

/// Default glyph side length, in logical px — the same default
/// [`crate::icon`] itself uses.
const DEFAULT_ICON_SIZE: f64 = 24.0;

/// Horizontal padding around the glyph used to derive the default square
/// slot when no explicit [`IconButtonView::slot_size`] is given — mirrors
/// [`crate::button`]'s `ButtonStyle::Icon` padding constants (same values,
/// for a consistent icon-affordance footprint across the crate).
const PAD_X: f64 = 12.0;
/// Vertical padding around the glyph — see [`PAD_X`].
const PAD_Y: f64 = 8.0;

/// Corner radius of the pressed wash, in logical px (the unthemed fallback;
/// a theme resolves this from `shape.small`, mirroring
/// [`crate::button`]'s `ButtonWidget::resolve_radius`).
const RADIUS: f64 = 6.0;

/// Unthemed default icon color — matches `icon.rs`'s own unthemed
/// `DEFAULT_COLOR` (the M3 light `on_surface` role), duplicated here since
/// that constant is private to its module; a theme resolves this from
/// `scheme().on_surface` instead (see the [module docs](self)).
const UNTHEMED_INK: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`crate::button`]'s identically-named helper).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A view-held, typed press callback (erased to [`ErasedCallback`] on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// Whether widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (the button's own bounds) — mirrors [`crate::button`]'s
/// identically-named helper.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Build the type-erased mark child view: [`crate::icon`] over `mark` at
/// `icon_size`, with `ink` applied as an explicit color override when set.
/// Shared by build/rebuild/teardown so the resolved child stays consistent
/// across the widget's whole lifecycle.
fn mark_view<State: 'static>(mark: IconData, icon_size: f64, ink: Option<Color>) -> AnyView<State> {
    let mut view = icon(mark).size(icon_size);
    if let Some(ink) = ink {
        view = view.color(ink);
    }
    any::<State, _>(view)
}

/// A declarative pressable vector icon. See the [module docs](self).
pub struct IconButtonView<State: 'static> {
    mark: IconData,
    label: String,
    /// `None` lets [`crate::icon`] resolve `on_surface` from the threaded
    /// theme (its own documented default) — see [`IconButtonView::ink`].
    ink: Option<Color>,
    icon_size: f64,
    /// `None` = the padding-derived square default — see the [module
    /// docs](self)'s "Slot and glyph sizing" section.
    slot_size: Option<f64>,
    on_press: OnPress<State>,
}

/// Create a pressable vector icon painting `mark`, labelled `label` for
/// accessibility, that runs `on_press` against the app state when released
/// inside its bounds. See the [module docs](self).
pub fn icon_button<State: 'static, F: Fn(&mut State) + 'static>(
    mark: impl Into<IconData>,
    label: impl Into<String>,
    on_press: F,
) -> IconButtonView<State> {
    IconButtonView {
        mark: mark.into(),
        label: label.into(),
        ink: None,
        icon_size: DEFAULT_ICON_SIZE,
        slot_size: None,
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`icon_button`], matching the container view-fn
/// vocabulary (see [`crate::Icon`]/[`crate::Button`]'s own aliases).
#[allow(non_snake_case)]
pub fn IconButton<State: 'static, F: Fn(&mut State) + 'static>(
    mark: impl Into<IconData>,
    label: impl Into<String>,
    on_press: F,
) -> IconButtonView<State> {
    icon_button(mark, label, on_press)
}

impl<State: 'static> IconButtonView<State> {
    /// Ink the mark explicitly, overriding the theme's `on_surface` default
    /// (explicit builder value wins — see the [module docs](self)).
    pub fn ink(mut self, ink: Color) -> Self {
        self.ink = Some(ink);
        self
    }

    /// Draw the glyph at `size` logical px (default [`DEFAULT_ICON_SIZE`]).
    pub fn icon_size(mut self, size: f64) -> Self {
        self.icon_size = size;
        self
    }

    /// Give the button an explicit square tap target, overriding the
    /// padding-derived default — see the [module docs](self)'s "Slot and
    /// glyph sizing" section.
    pub fn slot_size(mut self, size: f64) -> Self {
        self.slot_size = Some(size);
        self
    }
}

/// The retained widget for an [`IconButtonView`]. The mark is a nested
/// [`crate::IconWidget`] owned as a [`ChildPod`].
pub struct IconButtonWidget {
    mark: ChildPod,
    /// The accessible label (see [`IconButtonView`]'s field of the same
    /// name); the semantics node reads its name from here.
    label: String,
    ink: Option<Color>,
    icon_size: f64,
    slot_size: Option<f64>,
    /// The pressed *visual* state (the wash paints). Follows the cursor
    /// in/out while captured, and is purely cosmetic.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`. Gates all `Move`/`Up` handling so a hover `Move`
    /// (dispatched by the desktop shell on every cursor motion) never
    /// latches `pressed` or fires the callback without a preceding press.
    captured: bool,
    on_press: ErasedCallback,
}

impl IconButtonWidget {
    /// The pressed-wash corner radius: themed `shape.small`, resolved
    /// against the box so it never exceeds a pill (mirrors
    /// [`crate::button`]'s `ButtonWidget::resolve_radius`). Unthemed: the
    /// [`RADIUS`] constant exactly.
    fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
        match theme {
            Some(theme) => {
                frust_theme::ShapeScale::resolve(theme.shape.small, size.width, size.height)
            }
            None => RADIUS,
        }
    }

    /// The resolved ink color — mirrors `icon.rs`'s own `resolve_default_color`
    /// (duplicated locally since that helper is private to its module): an
    /// explicit [`IconButtonView::ink`] override wins; otherwise `on_surface`
    /// from the threaded theme, falling back to [`UNTHEMED_INK`].
    fn resolve_ink(&self, theme: Option<&Theme>) -> Color {
        if let Some(ink) = self.ink {
            return ink;
        }
        match theme {
            Some(theme) => theme.scheme().on_surface,
            None => UNTHEMED_INK,
        }
    }
}

impl<State: 'static> View<State> for IconButtonView<State> {
    type Element = IconButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> IconButtonWidget {
        let mark_view = mark_view::<State>(self.mark.clone(), self.icon_size, self.ink);
        IconButtonWidget {
            mark: crate::authoring::build_child(&mark_view, ctx),
            label: self.label.clone(),
            ink: self.ink,
            icon_size: self.icon_size,
            slot_size: self.slot_size,
            pressed: false,
            captured: false,
            on_press: erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut IconButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the adapter.
        element.on_press = erase_callback(&self.on_press);
        let prev_view = mark_view::<State>(prev.mark.clone(), prev.icon_size, prev.ink);
        let next_view = mark_view::<State>(self.mark.clone(), self.icon_size, self.ink);
        let mut flags =
            crate::authoring::rebuild_child(&prev_view, &next_view, &mut element.mark, ctx);
        if prev.icon_size != self.icon_size || prev.slot_size != self.slot_size {
            element.icon_size = self.icon_size;
            element.slot_size = self.slot_size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.ink != self.ink {
            element.ink = self.ink;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            // Semantics are recomputed off the paint pass, so a label change
            // republishes through PAINT (mirrors the muxr topbar's
            // `IconButtonWidget::rebuild` precedent).
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut IconButtonWidget, ctx: &mut BuildCtx<'_>) {
        let mark_view = mark_view::<State>(self.mark.clone(), self.icon_size, self.ink);
        crate::authoring::teardown_child(&mark_view, &mut element.mark, ctx);
    }
}

impl Widget for IconButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A tight constraint always wins (mirrors `IconWidget::layout`), so
        // `mark_size` is exactly `(icon_size, icon_size)`.
        let icon_bc = BoxConstraints::tight(Size::new(self.icon_size, self.icon_size));
        let mark_size = self.mark.layout_child(ctx, &icon_bc);
        let side = self.slot_size.unwrap_or_else(|| {
            let width = mark_size.width + PAD_X * 2.0;
            let height = mark_size.height + PAD_Y * 2.0;
            width.max(height)
        });
        let size = bc.constrain(Size::new(side, side));
        self.mark.set_origin(Point::new(
            (size.width - mark_size.width) / 2.0,
            (size.height - mark_size.height) / 2.0,
        ));
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let ink = self.resolve_ink(theme);
        let radius = Self::resolve_radius(theme, ctx.size());
        self.mark.paint_child(ctx, scene);
        if self.pressed {
            // Transparent at rest (module docs): only a pressed wash paints,
            // in the mark's own resolved ink so a light-on-dark slot and a
            // dark-on-light one both darken correctly.
            scene.fill_rounded_rect(
                ctx.origin(),
                ctx.size(),
                radius,
                with_alpha(ink, PRESSED_OPACITY),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Only a press we armed on `Down` tracks the cursor; a hover
                // `Move` (no prior press) is not ours.
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = inside(p.position, ctx.size());
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Fire on up-inside only.
                if inside(p.position, ctx.size()) {
                    (self.on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
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
        // A single a11y node (Role::Button) labelled by the accessible name —
        // the mark itself contributes nothing separately (an icon is
        // decorative on its own; the *name* of the control is what a screen
        // reader needs, never the shape of its glyph).
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label.as_str());
            node.add_action(Action::Click);
        });
    }

    crate::authoring::visit_children!(mark);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    /// Build an icon-button widget over `Counter` state, with a known
    /// 48×48 size (the default slot for a 24px glyph) for the inside/outside
    /// geometry.
    fn widget() -> IconButtonWidget {
        let view = icon_button::<Counter, _>(crate::icons::CLOSE, "Close", |s: &mut Counter| {
            s.presses += 1
        });
        let mut counter = 0u64;
        View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Secondary,
        })
    }

    fn dispatch(w: &mut IconButtonWidget, state: &mut Counter, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(48.0, 48.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn down_then_up_inside_fires_once() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        assert_eq!(state.presses, 1);
        assert!(!w.pressed);
    }

    #[test]
    fn down_inside_move_out_up_outside_does_not_fire() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 200.0, 10.0));
        assert!(!w.pressed, "moving out clears the pressed wash");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 10.0));
        assert_eq!(state.presses, 0, "up outside must not fire");
    }

    #[test]
    fn cancel_clears_pressed_without_firing() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(!w.pressed);
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn move_back_inside_then_up_fires() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 200.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 20.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 10.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn up_without_down_does_not_fire() {
        let mut w = widget();
        let mut state = Counter::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(48.0, 48.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Up, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(state.presses, 0, "an unarmed Up must never fire");
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget();
        let mut state = Counter::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(48.0, 48.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed, "hover must not press");
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn a_secondary_press_neither_presses_nor_captures_nor_fires() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Down, 10.0, 10.0),
        );
        assert!(!w.pressed, "no pressed wash on a right-click");
        assert!(!w.captured, "and no capture for the shell to wedge on");
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Up, 10.0, 10.0),
        );
        assert_eq!(state.presses, 0);

        // The primary gesture is untouched by the guard.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn semantics_publishes_a_single_button_node_named_by_the_label() {
        // Drives a real `RenderRoot` (the radio/checkbox precedent) rather
        // than constructing a `SemanticsCtx` directly — its constructor
        // isn't public, and the semantics tree is only meaningful once
        // published through a root.
        fn logic(_s: &mut ()) -> IconButtonView<()> {
            icon_button::<(), _>(crate::icons::CLOSE, "Close", |_: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), IconButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let matches: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(matches.len(), 1, "one a11y node, not one per glyph path");
        let (_, node) = matches[0];
        assert_eq!(node.label(), Some("Close"));
        assert!(node.supports_action(Action::Click));
    }

    // --- layout: slot/glyph sizing ------------------------------------------

    #[test]
    fn default_slot_squares_a_24px_glyph_to_48px() {
        let mut w = widget();
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(48.0, 48.0));
    }

    #[test]
    fn explicit_slot_size_overrides_the_padding_derived_default() {
        let view = icon_button::<Counter, _>(crate::icons::CLOSE, "Close", |_: &mut Counter| {})
            .slot_size(30.0);
        let mut counter = 0u64;
        let mut w = View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(30.0, 30.0));
    }

    #[test]
    fn explicit_icon_size_grows_the_default_slot() {
        let view = icon_button::<Counter, _>(crate::icons::CLOSE, "Close", |_: &mut Counter| {})
            .icon_size(32.0);
        let mut counter = 0u64;
        let mut w = View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        // 32 + 2*PAD_X = 56, 32 + 2*PAD_Y = 48 -> the wider axis wins.
        assert_eq!(size, Size::new(56.0, 56.0));
    }
}
