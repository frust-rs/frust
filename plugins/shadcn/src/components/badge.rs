//! Ports shadcn/ui's **Badge** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/badge.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a `rounded-full` pill,
//! `variant` × 6 (`default`/`secondary`/`destructive`/`outline`/`ghost`/`link`).
//!
//! # Static by default, interactive when clicked
//!
//! Upstream's hover/focus classes are all scoped `[a&]:`/`focus-visible:` —
//! they only apply when the badge renders as an anchor (`asChild` wrapping a
//! link). This port mirrors that split structurally: a plain [`badge`] is an
//! inert leaf (no hover claim, no focus, no cursor — a screen-reader `Status`
//! chip and nothing else); [`BadgeView::on_click`] turns it into the pressable
//! control the source's anchor form becomes, at which point the full hover/
//! press/focus/cursor contract switches on.
//!
//! # Deviation: `link` variant paints no underline
//!
//! `hover:underline` has no stroke-primitive counterpart cheap enough to be
//! worth inventing for one variant; the `link` badge keeps its `text-primary`
//! ink on hover with no visual press/hover feedback beyond whatever the
//! shared focus ring or cursor already provide.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, RoundedRect,
    SemanticsCtx, Shape, Size, Vec2, View, Widget, erase_callback,
};

use crate::hit::{inside, presses};
use crate::style::{
    ACTIVE_CURSOR, BORDER_WIDTH, HOVER_SOLID_ALPHA, PATH_TOLERANCE, TEXT_XS, draw_focus_ring,
    focus_border, ring_color, with_alpha,
};
use crate::text::Label;

/// Horizontal padding inside the pill (`px-2` = `spacing(2)`), in logical px.
const PAD_X: f64 = 8.0;
/// Vertical padding inside the pill (`py-0.5` = `spacing(0.5)`), in logical px.
const PAD_Y: f64 = 2.0;

/// cva `variant` axis. `Default` is shadcn's own `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeVariant {
    /// `bg-primary text-primary-foreground`.
    #[default]
    Default,
    /// `bg-secondary text-secondary-foreground`.
    Secondary,
    /// `bg-destructive text-white`.
    Destructive,
    /// `border-border text-foreground`, transparent fill.
    Outline,
    /// No fill/border at rest, `hover:bg-accent`.
    Ghost,
    /// `text-primary underline-offset-4`, no fill/border.
    Link,
}

/// `(fill, ink, border)` at rest, and the interactive-only `(fill, ink)`
/// swap (`None` for `Link`, which paints no swap — see the [module
/// docs](self)).
struct BadgePaint {
    fill: Color,
    ink: Color,
    border: Color,
    active: Option<(Color, Color)>,
}

impl BadgeVariant {
    /// Resolve this variant's paint under `theme`, or the built-in
    /// [`Theme::neutral`] role mapping when no theme is threaded (a badge has
    /// no local fallback constants of its own — every color it needs already
    /// has a `ColorScheme` role, unlike a leaf that needs an out-of-band
    /// accent).
    fn resolve(self, theme: Option<&Theme>) -> BadgePaint {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(crate::tokens::color_scheme_light().$role, |s| s.$role)
            };
        }
        match self {
            BadgeVariant::Default => BadgePaint {
                fill: role!(primary),
                ink: role!(on_primary),
                border: Color::TRANSPARENT,
                active: Some((
                    with_alpha(role!(primary), HOVER_SOLID_ALPHA),
                    role!(on_primary),
                )),
            },
            BadgeVariant::Secondary => BadgePaint {
                fill: role!(secondary),
                ink: role!(on_secondary),
                border: Color::TRANSPARENT,
                active: Some((
                    with_alpha(role!(secondary), HOVER_SOLID_ALPHA),
                    role!(on_secondary),
                )),
            },
            BadgeVariant::Destructive => BadgePaint {
                fill: role!(error),
                ink: role!(on_error),
                border: Color::TRANSPARENT,
                active: Some((with_alpha(role!(error), HOVER_SOLID_ALPHA), role!(on_error))),
            },
            BadgeVariant::Outline => BadgePaint {
                fill: Color::TRANSPARENT,
                ink: role!(on_surface),
                border: role!(outline),
                active: Some((role!(primary_container), role!(on_primary_container))),
            },
            BadgeVariant::Ghost => BadgePaint {
                fill: Color::TRANSPARENT,
                ink: role!(on_surface),
                border: Color::TRANSPARENT,
                active: Some((role!(primary_container), role!(on_primary_container))),
            },
            BadgeVariant::Link => BadgePaint {
                fill: Color::TRANSPARENT,
                ink: role!(primary),
                border: Color::TRANSPARENT,
                active: None,
            },
        }
    }
}

/// A view-held, typed click callback (erased to [`ErasedCallback`] on build).
type OnClick<State> = std::rc::Rc<dyn Fn(&mut State)>;

/// A declarative shadcn badge.
pub struct BadgeView<State: 'static> {
    label: String,
    variant: BadgeVariant,
    on_click: Option<OnClick<State>>,
}

/// Create a badge labelled `label`, [`BadgeVariant::Default`], static (no
/// click).
pub fn badge<State: 'static>(label: impl Into<String>) -> BadgeView<State> {
    BadgeView {
        label: label.into(),
        variant: BadgeVariant::default(),
        on_click: None,
    }
}

impl<State: 'static> BadgeView<State> {
    /// Select the cva `variant` (default [`BadgeVariant::Default`]).
    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Make this badge pressable — switches on the interactive contract (hover,
    /// focus, cursor, press) the [module docs](self) describe.
    pub fn on_click(mut self, on_click: impl Fn(&mut State) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(on_click));
        self
    }
}

/// The retained widget for a [`BadgeView`].
pub struct BadgeWidget {
    label: Label,
    label_text: String,
    variant: BadgeVariant,
    interactive: bool,
    on_click: Option<ErasedCallback>,
    /// Hit-test-latched hover flag; self-corrected from `PaintCtx::is_hovered`
    /// every paint (the three-part hover contract).
    hovered: bool,
    pressed: bool,
    captured: bool,
}

impl<State: 'static> View<State> for BadgeView<State> {
    type Element = BadgeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BadgeWidget {
        BadgeWidget {
            label: Label::new(self.label.clone()),
            label_text: self.label.clone(),
            variant: self.variant,
            interactive: self.on_click.is_some(),
            on_click: self.on_click.as_ref().map(erase_callback),
            hovered: false,
            pressed: false,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BadgeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
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
        element.interactive = self.on_click.is_some();
        element.on_click = self.on_click.as_ref().map(erase_callback);
        if !element.interactive {
            element.hovered = false;
            element.pressed = false;
            element.captured = false;
        }
        flags
    }
}

impl Widget for BadgeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let paint = self.variant.resolve(theme);
        let style = TextStyle {
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(TEXT_XS as f32, paint.ink)
        };
        let label_size = self.label.layout(ctx, &style);
        let width = label_size.width + PAD_X * 2.0;
        let height = label_size.height + PAD_Y * 2.0;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let paint = self.variant.resolve(theme);
        let radius = ctx.size().height / 2.0;

        if self.interactive {
            // Self-correct against the authoritative paint-time hover read —
            // the pointer's departure never reaches `event`.
            self.hovered = ctx.is_hovered();
        }
        let active = self.interactive && (self.hovered || self.pressed);
        let fill = match (active, paint.active) {
            (true, Some((swap_fill, _))) => swap_fill,
            _ => paint.fill,
        };

        if fill != Color::TRANSPARENT {
            scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        }
        let border = if self.interactive && ctx.has_focus() {
            focus_border(paint.border, true, theme)
        } else {
            paint.border
        };
        if border != Color::TRANSPARENT {
            let half = BORDER_WIDTH / 2.0;
            let size = ctx.size();
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (radius - half).max(0.0),
            );
            scene.stroke_path(
                ctx.origin(),
                &rr.to_path(PATH_TOLERANCE),
                BORDER_WIDTH,
                &Brush::Solid(border),
            );
        }
        if self.interactive && ctx.has_focus() {
            let ring = ring_color(None, theme);
            draw_focus_ring(scene, ctx.origin(), ctx.size(), radius, ring);
        }

        // The label ink stays at its resting (layout-time-baked) color even
        // when `active` swaps the fill: `PaintCtx` carries no text-shaping
        // context (see this catalog's other leaf-text components' docs), and
        // hover/press are paint-self-corrected runtime flags outside the
        // `View`/`ChangeFlags` diffing loop that bakes a shaped run's color —
        // forcing a relayout from either would fight that contract. Only
        // `Outline`/`Ghost`'s hover ink (`on_primary_container`) is affected in
        // practice, and the fill swap alone still reads as the active state.
        let label_size = self.label.size();
        let label_origin = ctx.origin()
            + Vec2::new(
                (ctx.size().width - label_size.width) / 2.0,
                (ctx.size().height - label_size.height) / 2.0,
            );
        self.label.paint(label_origin, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
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
                if inside(p.position, size)
                    && let Some(on_click) = &mut self.on_click
                {
                    (on_click)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.hovered = false;
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
        if self.interactive {
            ctx.push_node(Role::Button, |node| {
                node.set_label(self.label_text.as_str());
                node.add_action(Action::Click);
            });
        } else {
            ctx.push_node(Role::Status, |node| {
                node.set_label(self.label_text.as_str());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::Point;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    #[derive(Default)]
    struct Clicks {
        count: u32,
    }

    fn build_static() -> BadgeWidget {
        let view: BadgeView<Clicks> = badge("New");
        let mut counter = 0u64;
        View::<Clicks>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn build_clickable() -> BadgeWidget {
        let view: BadgeView<Clicks> = badge("New").on_click(|s: &mut Clicks| s.count += 1);
        let mut counter = 0u64;
        View::<Clicks>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut BadgeWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
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
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn paint(w: &mut BadgeWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut BadgeWidget, state: &mut Clicks, size: Size, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    #[test]
    fn unthemed_paint_uses_the_default_variant_fallback() {
        let mut w = build_static();
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, crate::tokens::color_scheme_light().primary);
        assert_eq!(rec.rrects[0].2, size.height / 2.0, "rounded-full pill");
    }

    #[test]
    fn themed_paint_resolves_the_secondary_role() {
        let theme = crate::tokens::theme();
        let view: BadgeView<()> = badge("New").variant(BadgeVariant::Secondary);
        let mut w = {
            let mut counter = 0u64;
            View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
        };
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().secondary);
        assert_eq!(rec.glyph_colors[0], theme.scheme().on_secondary);
    }

    #[test]
    fn outline_variant_paints_a_border_and_no_fill() {
        let theme = crate::tokens::theme();
        let view: BadgeView<()> = badge("New").variant(BadgeVariant::Outline);
        let mut w = {
            let mut counter = 0u64;
            View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
        };
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert!(rec.rrects.is_empty(), "transparent fill paints no rect");
        assert_eq!(rec.strokes[0], theme.scheme().outline);
    }

    #[test]
    fn a_static_badge_ignores_pointer_events() {
        let mut w = build_static();
        let size = layout(&mut w, None);
        let mut state = Clicks::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 2.0, 2.0));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 2.0, 2.0));
        assert_eq!(state.count, 0);
        assert!(!w.captured);
    }

    #[test]
    fn a_clickable_badge_fires_on_up_inside() {
        let mut w = build_clickable();
        let size = layout(&mut w, None);
        let mut state = Clicks::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 2.0, 2.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 2.0, 2.0));
        assert_eq!(state.count, 1);
        assert!(!w.pressed);
    }

    #[test]
    fn a_clickable_badge_does_not_fire_on_up_outside() {
        let mut w = build_clickable();
        let size = layout(&mut w, None);
        let mut state = Clicks::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 2.0, 2.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, size.width + 40.0, 2.0),
        );
        assert_eq!(state.count, 0);
    }

    #[test]
    fn cancel_clears_the_press_without_firing() {
        let mut w = build_clickable();
        let size = layout(&mut w, None);
        let mut state = Clicks::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 2.0, 2.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 2.0, 2.0),
        );
        assert!(!w.pressed);
        assert!(!w.captured);
        assert_eq!(state.count, 0);
    }

    #[test]
    fn hover_move_latches_on_a_clickable_badge_without_a_prior_down() {
        let mut w = build_clickable();
        let size = layout(&mut w, None);
        let mut state = Clicks::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Move, 2.0, 2.0));
        assert!(w.hovered);
    }

    #[test]
    fn hover_move_is_a_noop_on_a_static_badge() {
        let mut w = build_static();
        let size = layout(&mut w, None);
        let mut state = Clicks::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 2.0, 2.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.hovered);
        assert!(!ctx.needs_redraw());
    }

    #[test]
    fn pressed_swaps_the_fill_at_the_shared_hover_alpha() {
        let theme = crate::tokens::theme();
        let mut w = build_clickable();
        let size = layout(&mut w, Some(&theme));
        w.pressed = true;
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(theme.scheme().primary, HOVER_SOLID_ALPHA)
        );
    }
}
