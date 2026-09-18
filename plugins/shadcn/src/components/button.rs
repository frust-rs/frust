//! Ports shadcn/ui's **Button** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/button.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — `variant` × 6, `size` × 7
//! (`default`/`xs`/`sm`/`lg`/`icon`/`icon-sm`/`icon-lg`; `icon-xs` is not
//! ported — a deliberate scope cut).
//!
//! # Press reuses the hover swap
//!
//! The source has no `active:`-class beyond what `hover:`/`focus-visible:`
//! already cover — there is no separate pressed look to port. This button
//! therefore paints the same swapped fill/ink for "hovered or captured with
//! the pointer still inside" (`active`, below), matching shadcn's own visual
//! (a press is just a sustained hover on a device with no real hover) rather
//! than inventing a third state.
//!
//! # Only `Outline` ever shows a border
//!
//! `focus-visible:border-ring` is a shared base class, but only the
//! `outline` variant's class list also carries a `border` **width** utility
//! — Tailwind's border-color classes are inert without one. So every other
//! variant's border stays invisible at every state; only `Outline` paints a
//! stroke (`colors.outline` at rest, swapped to the ring color on focus).
//! The outer focus ring ([`crate::style::draw_focus_ring`]) is independent of
//! this and paints for every variant.
//!
//! # `disabled` bakes into the shaped label
//!
//! [`ButtonView::disabled`] dims fill/border/ink alike via
//! [`crate::style::disabled_tint`]; since the ink is baked into the label's
//! shaped run, a `disabled` flip is flagged `LAYOUT | PAINT` (not `PAINT`
//! alone) so the run re-shapes at the dimmed color — the same layout-time-
//! baked-color contract `sample_design::badge`'s docs describe. `disabled`
//! also fully suppresses the event pass (`disabled:pointer-events-none`): no
//! claims, no cursor, no focus.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, RoundedRect,
    SemanticsCtx, Shape, Size, ThemeTextType, Vec2, View, Widget, erase_callback,
};

use crate::hit::{inside, presses};
use crate::style::{
    ACTIVE_CURSOR, BORDER_WIDTH, HOVER_SECONDARY_ALPHA, HOVER_SOLID_ALPHA, PATH_TOLERANCE,
    SHADOW_XS, TEXT_SM, TEXT_XS, disabled_tint, draw_focus_ring, draw_shadow, focus_border,
    ring_color, with_alpha,
};
use crate::text::{Label, themed_family};
use crate::tokens::ShadcnTokens;

/// cva `variant` axis. `Default` is shadcn's own `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// `bg-primary text-primary-foreground hover:bg-primary/90`.
    #[default]
    Default,
    /// `bg-destructive text-white hover:bg-destructive/90`.
    Destructive,
    /// `border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground`.
    Outline,
    /// `bg-secondary text-secondary-foreground hover:bg-secondary/80`.
    Secondary,
    /// Transparent at rest, `hover:bg-accent hover:text-accent-foreground`.
    Ghost,
    /// `text-primary underline-offset-4 hover:underline` — no fill ever (see
    /// the [module docs](self) for the un-ported underline).
    Link,
}

/// cva `size` axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// `h-9 px-4 py-2` (36px).
    #[default]
    Default,
    /// `h-6 gap-1 px-2 text-xs` (24px).
    Xs,
    /// `h-8 gap-1.5 px-3` (32px).
    Sm,
    /// `h-10 px-6` (40px).
    Lg,
    /// `size-9` — a fixed 36px square.
    Icon,
    /// `size-8` — a fixed 32px square.
    IconSm,
    /// `size-10` — a fixed 40px square.
    IconLg,
}

impl ButtonSize {
    /// The fixed edge/height, in logical px.
    fn edge(self) -> f64 {
        match self {
            ButtonSize::Default => crate::style::HEIGHT_DEFAULT,
            ButtonSize::Xs => crate::style::HEIGHT_XS,
            ButtonSize::Sm => crate::style::HEIGHT_SM,
            ButtonSize::Lg => crate::style::HEIGHT_LG,
            ButtonSize::Icon => crate::style::HEIGHT_DEFAULT,
            ButtonSize::IconSm => crate::style::HEIGHT_SM,
            ButtonSize::IconLg => crate::style::HEIGHT_LG,
        }
    }

    /// Whether this is one of the fixed-square `icon*` sizes.
    fn is_icon(self) -> bool {
        matches!(
            self,
            ButtonSize::Icon | ButtonSize::IconSm | ButtonSize::IconLg
        )
    }

    /// Horizontal padding around the label — `0.0` for the fixed-square icon
    /// sizes.
    fn pad_x(self) -> f64 {
        match self {
            ButtonSize::Default => 16.0,
            ButtonSize::Xs => 8.0,
            ButtonSize::Sm => 12.0,
            ButtonSize::Lg => 24.0,
            ButtonSize::Icon | ButtonSize::IconSm | ButtonSize::IconLg => 0.0,
        }
    }

    /// Label font size — `text-xs` under `Xs`, `text-sm` everywhere else.
    fn font_size(self) -> f64 {
        if matches!(self, ButtonSize::Xs) {
            TEXT_XS
        } else {
            TEXT_SM
        }
    }
}

/// The resolved `(fill, ink, border, active swap, shadow)` for a variant —
/// see [`ButtonVariant::resolve`].
struct ButtonPaint {
    fill: Color,
    ink: Color,
    border: Option<Color>,
    /// `(fill, ink)` while hovered/pressed — `None` for `Link`.
    active: Option<(Color, Color)>,
    shadow: bool,
}

impl ButtonVariant {
    fn resolve(self, theme: Option<&Theme>) -> ButtonPaint {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(crate::tokens::color_scheme_light().$role, |s| s.$role)
            };
        }
        match self {
            ButtonVariant::Default => ButtonPaint {
                fill: role!(primary),
                ink: role!(on_primary),
                border: None,
                active: Some((
                    with_alpha(role!(primary), HOVER_SOLID_ALPHA),
                    role!(on_primary),
                )),
                shadow: false,
            },
            ButtonVariant::Destructive => ButtonPaint {
                fill: role!(error),
                ink: role!(on_error),
                border: None,
                active: Some((with_alpha(role!(error), HOVER_SOLID_ALPHA), role!(on_error))),
                shadow: false,
            },
            ButtonVariant::Outline => ButtonPaint {
                fill: role!(surface),
                ink: role!(on_surface),
                border: Some(role!(outline)),
                active: Some((role!(primary_container), role!(on_primary_container))),
                shadow: true,
            },
            ButtonVariant::Secondary => ButtonPaint {
                fill: role!(secondary),
                ink: role!(on_secondary),
                border: None,
                active: Some((
                    with_alpha(role!(secondary), HOVER_SECONDARY_ALPHA),
                    role!(on_secondary),
                )),
                shadow: false,
            },
            ButtonVariant::Ghost => ButtonPaint {
                fill: Color::TRANSPARENT,
                ink: role!(on_surface),
                border: None,
                active: Some((role!(primary_container), role!(on_primary_container))),
                shadow: false,
            },
            ButtonVariant::Link => ButtonPaint {
                fill: Color::TRANSPARENT,
                ink: role!(primary),
                border: None,
                active: None,
                shadow: false,
            },
        }
    }
}

/// A declarative shadcn button.
pub struct ButtonView<State: 'static> {
    label: String,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    on_press: std::rc::Rc<dyn Fn(&mut State)>,
}

/// Create a button labelled `label` that runs `on_press` against the app
/// state when released inside its bounds.
pub fn button<State: 'static>(
    label: impl Into<String>,
    on_press: impl Fn(&mut State) + 'static,
) -> ButtonView<State> {
    ButtonView {
        label: label.into(),
        variant: ButtonVariant::default(),
        size: ButtonSize::default(),
        disabled: false,
        on_press: std::rc::Rc::new(on_press),
    }
}

impl<State: 'static> ButtonView<State> {
    /// Select the cva `variant` (default [`ButtonVariant::Default`]).
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Select the cva `size` (default [`ButtonSize::Default`]).
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Disable the button: suppresses `on_press`, blocks focus/hover/cursor
    /// claims, and dims fill/border/ink by [`crate::style::DISABLED_OPACITY`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The retained widget for a [`ButtonView`].
pub struct ButtonWidget {
    label: Label,
    label_text: String,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    on_press: ErasedCallback,
    hovered: bool,
    pressed: bool,
    captured: bool,
}

impl<State: 'static> View<State> for ButtonView<State> {
    type Element = ButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ButtonWidget {
        ButtonWidget {
            label: Label::new(self.label.clone()),
            label_text: self.label.clone(),
            variant: self.variant,
            size: self.size,
            disabled: self.disabled,
            on_press: erase_callback(&self.on_press),
            hovered: false,
            pressed: false,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            element.label_text = self.label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            // The disabled ink is baked into the shaped run — see the
            // [module docs](self).
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            if self.disabled {
                element.pressed = false;
                element.captured = false;
                element.hovered = false;
            }
        }
        flags
    }
}

impl Widget for ButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let paint = self.variant.resolve(theme);
        let ink = disabled_tint(paint.ink, self.disabled);
        let style = themed_family(
            TextStyle {
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(self.size.font_size() as f32, ink)
            },
            theme,
            ThemeTextType::LabelLarge,
        );
        let label_size = self.label.layout(ctx, &style);

        if self.size.is_icon() {
            let edge = self.size.edge();
            return bc.constrain(Size::new(edge, edge));
        }
        let pad_x = self.size.pad_x();
        let width = label_size.width + pad_x * 2.0;
        bc.constrain(Size::new(width, self.size.edge()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let paint = self.variant.resolve(theme);
        let radius = ShadcnTokens::resolve_radius(None, theme).md;
        let size = ctx.size();
        let origin = ctx.origin();

        if !self.disabled {
            self.hovered = ctx.is_hovered();
        }
        let active = !self.disabled && (self.hovered || self.pressed);
        let (mut fill, _swap_ink) = match (active, paint.active) {
            (true, Some(swap)) => swap,
            _ => (paint.fill, paint.ink),
        };
        let mut border = paint.border;

        if self.disabled {
            fill = disabled_tint(fill, true);
            border = border.map(|b| disabled_tint(b, true));
        }

        if paint.shadow && !self.disabled {
            draw_shadow(scene, origin, size, radius, SHADOW_XS, theme);
        }
        if fill != Color::TRANSPARENT {
            scene.fill_rounded_rect(origin, size, radius, fill);
        }

        let focused = !self.disabled && ctx.has_focus();
        if let Some(base_border) = border {
            let resolved = if focused {
                focus_border(base_border, true, theme)
            } else {
                base_border
            };
            let half = BORDER_WIDTH / 2.0;
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (radius - half).max(0.0),
            );
            scene.stroke_path(
                origin,
                &rr.to_path(PATH_TOLERANCE),
                BORDER_WIDTH,
                &Brush::Solid(resolved),
            );
        }
        if focused {
            let ring = ring_color(None, theme);
            draw_focus_ring(scene, origin, size, radius, ring);
        }

        let label_size = self.label.size();
        let label_origin = origin
            + Vec2::new(
                (size.width - label_size.width) / 2.0,
                (size.height - label_size.height) / 2.0,
            );
        self.label.paint(label_origin, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.disabled {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured {
                    let now_inside = inside(p.position, size);
                    self.pressed = now_inside;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                let over = inside(p.position, size);
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, size) {
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
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::Point;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn build(view: &ButtonView<Counter>) -> ButtonWidget {
        let mut counter = 0u64;
        View::<Counter>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    /// The same event on the secondary (right) button.
    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Secondary,
        })
    }

    fn dispatch(w: &mut ButtonWidget, state: &mut Counter, size: Size, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn layout(w: &mut ButtonWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        shadows: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(
            &mut self,
            _o: Point,
            _p: &frust::authoring::BezPath,
            _w: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _std: f64, color: Color) {
            self.shadows.push(color);
        }
    }

    fn paint(w: &mut ButtonWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn default_size_is_h9_with_px4() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        assert_eq!(size.height, crate::style::HEIGHT_DEFAULT);
    }

    #[test]
    fn icon_sizes_are_fixed_squares() {
        let view = button::<Counter>("+", |_| {}).size(ButtonSize::Icon);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        assert_eq!(
            size,
            Size::new(crate::style::HEIGHT_DEFAULT, crate::style::HEIGHT_DEFAULT)
        );
    }

    #[test]
    fn down_then_up_inside_fires_once() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 1);
        assert!(!w.pressed);
    }

    #[test]
    fn up_outside_does_not_fire() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, size.width + 40.0, 5.0),
        );
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn cancel_clears_without_firing() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 5.0, 5.0),
        );
        assert!(!w.captured);
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn a_secondary_press_neither_presses_nor_captures_nor_fires() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let mut state = Counter::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &secondary_ev(PointerPhase::Down, 5.0, 5.0),
        );
        assert!(!w.pressed, "no pressed chrome on a right-click");
        assert!(!w.captured, "and no capture to wedge the shell with");
        dispatch(
            &mut w,
            &mut state,
            size,
            &secondary_ev(PointerPhase::Up, 5.0, 5.0),
        );
        assert_eq!(state.presses, 0);

        // The primary gesture is untouched by the guard.
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn hover_move_claims_and_latches_without_a_prior_down() {
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Move, 5.0, 5.0));
        assert!(w.hovered);
    }

    #[test]
    fn disabled_ignores_every_pointer_event() {
        let view = button::<Counter>("Go", |s| s.presses += 1).disabled(true);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 0);
        assert!(!w.captured);
    }

    #[test]
    fn unthemed_default_variant_paints_the_fallback_role() {
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, crate::tokens::color_scheme_light().primary);
    }

    #[test]
    fn themed_default_variant_resolves_primary_and_swaps_while_pressed() {
        // `hovered` is self-corrected from `PaintCtx::is_hovered()` every
        // paint (the authoritative read — see the module docs' three-part
        // hover contract), which a bare `PaintCtx::new` always reports
        // `false` for outside a real `RenderRoot` dispatch; `pressed` is a
        // widget-owned flag paint never touches, so it is what this test
        // exercises the active-swap branch through.
        let theme = crate::tokens::theme();
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);

        w.pressed = true;
        let rec2 = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec2.rrects[0].3,
            with_alpha(theme.scheme().primary, HOVER_SOLID_ALPHA)
        );
    }

    #[test]
    fn outline_variant_paints_border_and_shadow_xs() {
        let theme = crate::tokens::theme();
        let view = button::<Counter>("Go", |_| {}).variant(ButtonVariant::Outline);
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.strokes[0], theme.scheme().outline);
        assert!(!rec.shadows.is_empty());
    }

    #[test]
    fn other_variants_never_paint_a_border() {
        let theme = crate::tokens::theme();
        for variant in [
            ButtonVariant::Default,
            ButtonVariant::Destructive,
            ButtonVariant::Secondary,
            ButtonVariant::Ghost,
            ButtonVariant::Link,
        ] {
            let view = button::<Counter>("Go", |_| {}).variant(variant);
            let mut w = build(&view);
            let size = layout(&mut w, Some(&theme));
            let rec = paint(&mut w, size, Some(&theme));
            assert!(
                rec.strokes.is_empty(),
                "{variant:?} must not paint a border"
            );
        }
    }

    #[test]
    fn disabled_dims_the_fill() {
        let theme = crate::tokens::theme();
        let view = button::<Counter>("Go", |_| {}).disabled(true);
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, disabled_tint(theme.scheme().primary, true));
    }

    // ---- Typeface: the label follows the live theme ----------------------

    /// Both label sizes: `text-sm`, and `Xs`'s `text-xs`.
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_label_paints_in_the_theme_face() {
        for size in [ButtonSize::Default, ButtonSize::Xs] {
            crate::text::typeface_probe::assert_paints_in_the_theme_face(
                &format!("a {size:?} button's label"),
                |_: &mut ()| button::<()>("Save", |_| {}).size(size),
            );
        }
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_label_follows_a_live_theme_swap() {
        for size in [ButtonSize::Default, ButtonSize::Xs] {
            crate::text::typeface_probe::assert_follows_a_live_theme_swap(
                &format!("a {size:?} button's label"),
                |_: &mut ()| button::<()>("Save", |_| {}).size(size),
            );
        }
    }
}
