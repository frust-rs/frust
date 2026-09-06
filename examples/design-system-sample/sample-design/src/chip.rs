//! [`sample_chip`]/[`SampleChipView`]: the design system's **interactive
//! control** — a pressable chip that fires an app-state callback.
//!
//! The third of the three authoring shapes (leaf: [`crate::badge`]; container:
//! [`crate::panel`]). What an interactive widget has to get right, and what
//! this one therefore demonstrates:
//!
//! - **callback erasure**: the view holds a typed `Rc<dyn Fn(&mut State)>`, the
//!   retained widget holds the `State`-erased
//!   [`ErasedCallback`](frust::authoring::ErasedCallback) that
//!   [`erase_callback`](frust::authoring::erase_callback) produces — which is
//!   what keeps [`SampleChipWidget`] non-generic over `State`. Closures aren't
//!   comparable, so `build`/`rebuild` reinstall the adapter unconditionally.
//! - **fire-on-up-inside**: `Down` inside captures the pointer and paints the
//!   pressed state, `Move` only tracks the pressed visual, the callback fires
//!   on `Up` **and only if the release lands inside**, and `Cancel` (a platform
//!   gesture steal) clears the state without firing. A `Cancel` arm never
//!   touches app state.
//! - **routing to the child first**: the chip owns a label child, so every
//!   event goes through
//!   [`route_event_single`](frust::authoring::route_event_single) before the
//!   chip considers handling it itself — that helper owns the capture/focus
//!   fast paths and the broadcast-forwarding rule a hand-rolled hit test would
//!   silently drop.
//!
//! # Token resolution
//!
//! The resting fill is `primary_container` (the bright accent *fill* role —
//! `primary` is the accent *ink*, and conflating the two is the built-in Glyph
//! catalog's most common accent bug). The pressed look is that same fill with
//! an `on_primary_container` state layer at
//! [`PRESSED_OPACITY`](frust::authoring::PRESSED_OPACITY) — the framework's one
//! language-neutral interaction opacity, imported rather than re-guessed. The
//! label rides the baseline `text` widget under the matching
//! `ThemeTextColor::OnPrimaryContainer` role, so a design system composing over
//! the baseline set is part of the proof too.

use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, ErasedCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PRESSED_OPACITY, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, ThemeTextColor, View, Widget,
    any, build_child, erase_callback, rebuild_child, route_event_single, teardown_child,
    visit_children,
};
use frust::{Theme, text};

/// Horizontal padding around the label, in logical px.
const PAD_X: f64 = 14.0;
/// Vertical padding around the label, in logical px.
const PAD_Y: f64 = 7.0;
/// Label size, in logical px.
const FONT_SIZE: f32 = 13.0;
/// Hairline border width, in logical px.
const BORDER_WIDTH: f64 = 1.0;
/// Flattening tolerance for the border's rounded-rect stroke path.
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed fallback fill (the Sample light-mode accent fill).
const FALLBACK_FILL: Color = Color::from_rgb8(0x0B, 0x4F, 0x4A);
/// Unthemed fallback state-layer ink (painted over the fill when pressed).
const FALLBACK_STATE_INK: Color = Color::from_rgb8(0xE6, 0xF4, 0xF2);
/// Unthemed fallback border.
const FALLBACK_BORDER: Color = Color::from_rgb8(0x0B, 0x4F, 0x4A);
/// Unthemed fallback corner radius, in logical px.
const FALLBACK_RADIUS: f64 = 3.0;

/// A declarative Sample chip. See the [module docs](self).
pub struct SampleChipView<State: 'static> {
    label: String,
    child: AnyView<State>,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// Create a chip labelled `label` that calls `on_press` on release inside its
/// bounds.
pub fn sample_chip<State: 'static>(
    label: impl Into<String>,
    on_press: impl Fn(&mut State) + 'static,
) -> SampleChipView<State> {
    let label = label.into();
    SampleChipView {
        child: any(text(label.clone())
            .size(FONT_SIZE)
            .themed_role(ThemeTextColor::OnPrimaryContainer)),
        label,
        on_press: Rc::new(on_press),
    }
}

/// The retained widget for a [`SampleChipView`]. Deliberately **not** generic
/// over `State` — that is what callback erasure buys.
pub struct SampleChipWidget {
    label: String,
    child: ChildPod,
    on_press: ErasedCallback,
    /// The pressed *visual*; follows the pointer in and out while captured.
    pressed: bool,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`.
    captured: bool,
}

impl<State: 'static> View<State> for SampleChipView<State> {
    type Element = SampleChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SampleChipWidget {
        SampleChipWidget {
            label: self.label.clone(),
            child: build_child(&self.child, ctx),
            on_press: erase_callback(&self.on_press),
            pressed: false,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SampleChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every
        // rebuild — cheap, and the only way a stale capture of app state can't
        // survive a rebuild.
        element.on_press = erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags | rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut SampleChipWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

/// `(fill, state_ink, border)` for the current theme, or the fallbacks.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.primary_container,
                scheme.on_primary_container,
                scheme.primary,
            )
        }
        None => (FALLBACK_FILL, FALLBACK_STATE_INK, FALLBACK_BORDER),
    }
}

/// The chip's corner radius: `theme.shape.small`, else the fallback.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(FALLBACK_RADIUS, |t| t.shape.small)
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Whether `point` (in this widget's local space) is inside a `size`-sized box.
fn inside(point: Point, size: Size) -> bool {
    Rect::from_origin_size(Point::ORIGIN, size).contains(point)
}

impl Widget for SampleChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let chrome = Size::new(PAD_X * 2.0, PAD_Y * 2.0);
        let child_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(
                (bc.max().width - chrome.width).max(0.0),
                (bc.max().height - chrome.height).max(0.0),
            ),
        );
        let child_size = self.child.layout_child(ctx, &child_bc);
        self.child.set_origin(Point::new(PAD_X, PAD_Y));
        bc.constrain(Size::new(
            child_size.width + chrome.width,
            child_size.height + chrome.height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (fill, state_ink, border) = resolve_colors(theme);
        let radius = resolve_radius(theme);

        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        if self.pressed {
            // A state layer over the resting fill, never a second hardcoded
            // "pressed" color — the framework publishes exactly one
            // language-neutral press opacity and this imports it.
            scene.fill_rounded_rect(
                ctx.origin(),
                ctx.size(),
                radius,
                with_alpha(state_ink, PRESSED_OPACITY),
            );
        }
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, ctx.size()), radius);
        scene.stroke_path(
            ctx.origin(),
            &rr.to_path(PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(border),
        );

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The child gets every event first — including broadcasts and
        // focus-routed events, which this helper forwards under their own
        // rules. Only what the child ignores can become a chip press.
        if route_event_single(&mut self.child, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Down => {
                if !inside(p.position, size) {
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
                self.pressed = inside(p.position, size);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Fire on up-INSIDE only; a release that drifted out is a
                // deliberate cancel by the user.
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
                // A Cancel arm clears internal flags and requests a redraw —
                // never app state, and never the callback.
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Button,
            |node| {
                node.set_label(self.label.as_str());
                node.add_action(Action::Click);
            },
            |ctx| self.child.semantics_child(ctx),
        );
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{RecordingScene, center, dispatch, layout_widget, paint_widget, pointer};
    use crate::tokens::sample_theme;

    #[derive(Default)]
    struct Presses {
        count: u32,
    }

    fn build() -> SampleChipWidget {
        let view: SampleChipView<Presses> = sample_chip("run", |s: &mut Presses| s.count += 1);
        let mut counter = 0u64;
        View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn render(widget: &mut SampleChipWidget, theme: Option<&Theme>) -> (Size, RecordingScene) {
        let size = layout_widget(widget, theme, Size::new(400.0, 100.0));
        let scene = paint_widget(widget, size, theme);
        (size, scene)
    }

    #[test]
    fn unthemed_resting_paint_uses_the_fallback_constants() {
        let mut w = build();
        let (_, rec) = render(&mut w, None);
        assert_eq!(rec.rounded_rects[0].3, FALLBACK_FILL);
        assert_eq!(rec.rounded_rects[0].2, FALLBACK_RADIUS);
        assert_eq!(rec.strokes[0], FALLBACK_BORDER);
    }

    #[test]
    fn themed_resting_paint_uses_the_accent_fill_not_the_accent_ink() {
        let theme = sample_theme();
        let mut w = build();
        let (_, rec) = render(&mut w, Some(&theme));
        assert_eq!(rec.rounded_rects[0].3, theme.scheme().primary_container);
        assert_ne!(rec.rounded_rects[0].3, theme.scheme().primary);
    }

    #[test]
    fn a_press_paints_a_state_layer_at_the_shared_press_opacity() {
        let mut w = build();
        let (size, resting) = render(&mut w, None);
        assert_eq!(resting.rounded_rects.len(), 1, "resting: fill only");

        let hit = center(size);
        let mut state = Presses::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, hit.x, hit.y),
        );
        assert!(w.pressed);

        let pressed = paint_widget(&mut w, size, None);
        assert_eq!(pressed.rounded_rects.len(), 2, "fill + state layer");
        let layer = pressed.rounded_rects[1].3;
        assert_eq!(layer.components[3], PRESSED_OPACITY);
    }

    #[test]
    fn up_inside_fires_the_erased_callback_once() {
        let mut w = build();
        let (size, _) = render(&mut w, None);
        let hit = center(size);
        let mut state = Presses::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, hit.x, hit.y),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, hit.x, hit.y),
        );

        assert_eq!(state.count, 1);
        assert!(!w.pressed);
        assert!(!w.captured);
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = build();
        let (size, _) = render(&mut w, None);
        let hit = center(size);
        let mut state = Presses::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, hit.x, hit.y),
        );
        // Release well past the right edge.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, size.width + 40.0, hit.y),
        );

        assert_eq!(state.count, 0);
        assert!(!w.captured);
    }

    #[test]
    fn cancel_clears_the_press_without_firing() {
        let mut w = build();
        let (size, _) = render(&mut w, None);
        let hit = center(size);
        let mut state = Presses::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, hit.x, hit.y),
        );
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, hit.x, hit.y),
        );

        assert_eq!(state.count, 0);
        assert!(!w.pressed);
        assert!(!w.captured);
    }

    #[test]
    fn a_down_outside_the_chip_never_arms_it() {
        let mut w = build();
        let (size, _) = render(&mut w, None);
        let mut state = Presses::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, size.width + 40.0, size.height + 40.0),
        );
        assert!(!w.captured);
    }

    #[test]
    fn visit_children_publishes_the_label_pod() {
        let mut w = build();
        let _ = render(&mut w, None);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }
}
