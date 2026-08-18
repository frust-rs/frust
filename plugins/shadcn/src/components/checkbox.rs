//! Ports shadcn/ui's **Checkbox** (the controlled check control) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/checkbox.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! The upstream class list is the whole design, and it translates like this:
//!
//! | class | here |
//! |---|---|
//! | `size-4` | [`CHECKBOX_SIZE`] |
//! | `rounded-[4px]` | [`CHECKBOX_RADIUS`] (an arbitrary value, *not* a radius-scale step) |
//! | `border border-input` | [`style::BORDER_WIDTH`] over `outline_variant` |
//! | `shadow-xs` | [`style::SHADOW_XS`] |
//! | `dark:bg-input/30` | [`DARK_FILL_ALPHA`] over `outline_variant`, dark only |
//! | `data-[state=checked]:bg-primary` + `:border-primary` | `primary` fill and border |
//! | `data-[state=checked]:text-primary-foreground` | the check/minus mark's `on_primary` stroke |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] |
//! | `disabled:opacity-50` + `disabled:cursor-not-allowed` | [`style::disabled_tint`] + [`style::DISABLED_CURSOR`] |
//!
//! # Controlled, never self-mutating
//!
//! `checked` is a prop and the app owns it: a release inside the box reports the
//! *requested* value through `on_checked_change(state, !checked)` and the widget
//! leaves its own `checked` alone until the next `rebuild` feeds the confirmed
//! value back down (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! # The indeterminate state, and where it diverges from the source
//!
//! Radix's primitive models a third value (`checked = "indeterminate"`) and
//! shadcn's own data-table passes it — `checked={allSelected || (someSelected &&
//! "indeterminate")}` in `blocks/dashboard-01/components/data-table.tsx`, same
//! rev — so [`CheckboxView::indeterminate`] carries it here. Two behaviors come
//! straight from the primitive, one is a deliberate divergence:
//!
//! - **Activating an indeterminate box reports `true`**, not `!checked` — Radix
//!   resolves a mixed state upward (`isIndeterminate(prev) ? true : !prev`), so a
//!   half-selected "select all" completes the selection rather than clearing it.
//! - **Semantics report [`Toggled::Mixed`]**, the ARIA `aria-checked="mixed"` the
//!   primitive emits.
//! - **The mark is a minus on the `primary` fill.** The source authors *no*
//!   `data-[state=indeterminate]` rule at all and renders one `CheckIcon` for both
//!   states, so its literal indeterminate rendering is an unfilled box carrying a
//!   check — indistinguishable from a partially-styled checked box, and it
//!   misreports "some of these are selected". This port paints lucide's
//!   `MinusIcon` (`M5 12h14`, the same glyph `input-otp.tsx` imports) on the same
//!   `primary` fill a checked box gets, which is the treatment the mixed state
//!   actually needs.
//!
//! # What the port deliberately does not carry
//!
//! - **No hover chrome.** The class list has no `hover:` variant — a checkbox
//!   acknowledges the pointer with the cursor, not a fill swap. The widget still
//!   claims the hover link from its uncaptured `Move` arm (so an enclosing
//!   card/row reads hovered through the path) but keeps no hover flag, because
//!   nothing it paints depends on one.
//! - **No `aria-invalid` ring.** The invalid treatment belongs to `field`, which
//!   owns validation state; this component has no error prop to key it off.
//!
//! # Focus is shown for a pointer press too
//!
//! CSS `:focus-visible` is keyboard-only; frust has no focus-visible heuristic
//! (a `Down` claims focus, and `PaintCtx::has_focus` is the only paint-time
//! read), so the ring appears after a click as well. Accepted catalog-wide
//! rather than reinvented per component.

use std::rc::Rc;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx,
    EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, Toggled, View, Widget, erase_callback_arg,
};
use frust::{Brightness, Theme};

use crate::hit::inside;
use crate::style::{self, PATH_TOLERANCE};

/// Box edge, in logical px (`size-4`).
pub const CHECKBOX_SIZE: f64 = 16.0;

/// Corner radius, in logical px (`rounded-[4px]`).
///
/// An arbitrary Tailwind value, so it is a constant here rather than a
/// [`ShadcnRadius`](crate::ShadcnRadius) step — the scale's nearest rung (`sm`,
/// 6px) is not what the source asks for.
pub const CHECKBOX_RADIUS: f64 = 4.0;

/// Mark box edge, in logical px (`size-3.5` on the `CheckIcon`, and on the
/// `MinusIcon` the indeterminate state substitutes for it).
const CHECK_SIZE: f64 = 14.0;

/// Side of the lucide icon viewBox the check path's coordinates are authored in
/// (`lucide-react`'s icons are all `0 0 24 24`).
const ICON_VIEWBOX: f64 = 24.0;

/// The check path's stroke width in viewBox units (`stroke-width="2"`, lucide's
/// uniform value) — scaled to [`CHECK_SIZE`] at paint time.
const CHECK_STROKE_VIEWBOX: f64 = 2.0;

/// Alpha of the unchecked fill in dark mode (`dark:bg-input/30`). Light mode
/// paints no fill at all, which is what the class list says.
const DARK_FILL_ALPHA: f32 = 0.30;

/// Unthemed fallback border/fill token — the `neutral` preset's light `--input`.
const FALLBACK_INPUT: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);
/// Unthemed fallback checked fill — the `neutral` preset's light `--primary`.
const FALLBACK_PRIMARY: Color = Color::from_rgb8(0x17, 0x17, 0x17);
/// Unthemed fallback check-mark ink — light `--primary-foreground`.
const FALLBACK_PRIMARY_FOREGROUND: Color = Color::from_rgb8(0xFA, 0xFA, 0xFA);

/// A view-held, typed change callback (erased on build).
type OnCheckedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative shadcn checkbox. See the [module docs](self).
pub struct CheckboxView<State: 'static> {
    checked: bool,
    indeterminate: bool,
    disabled: bool,
    label: Option<String>,
    on_checked_change: OnCheckedChange<State>,
}

/// Create a checkbox reflecting `checked` that reports
/// `on_checked_change(state, !checked)` on a release inside its bounds — a
/// **controlled** component (see the [module docs](self)).
///
/// shadcn pairs the box with a separate `<Label>` rather than nesting text in
/// it, and so does this port: compose the visible label alongside (a `Row` of
/// `checkbox(..)` plus `frust::text(..)`) and name it for assistive tech with
/// [`CheckboxView::label`].
pub fn checkbox<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> CheckboxView<State> {
    CheckboxView {
        checked,
        indeterminate: false,
        disabled: false,
        label: None,
        on_checked_change: Rc::new(on_checked_change),
    }
}

impl<State: 'static> CheckboxView<State> {
    /// Put the box in the mixed state: the `primary` fill under a minus mark,
    /// [`Toggled::Mixed`] semantics, and an activation that reports `true`
    /// whatever `checked` says (see the [module docs](self)).
    ///
    /// Takes precedence over `checked` for everything it paints, matching
    /// Radix's own value shape — `checked = "indeterminate"` is a *third* value
    /// there, not a flag beside the boolean.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    /// Disable the control: 50% opacity, inert to pointer and key, and a
    /// not-allowed cursor (`disabled:opacity-50 disabled:cursor-not-allowed`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech. The box paints no text of its own,
    /// so this is the only source for the semantics node's label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The resolved checkbox palette.
struct CheckboxColors {
    /// The unchecked border (`border-input`).
    border: Color,
    /// The checked fill and border (`bg-primary`/`border-primary`).
    primary: Color,
    /// The check mark (`text-primary-foreground`).
    mark: Color,
    /// The unchecked fill: `None` in light mode (no `bg-*` class),
    /// `Some(input/30)` in dark (`dark:bg-input/30`).
    unchecked_fill: Option<Color>,
}

/// Resolve the palette from the theme, falling back to the `neutral` preset's
/// light values with none threaded.
fn resolve_colors(theme: Option<&Theme>) -> CheckboxColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            CheckboxColors {
                border: scheme.outline_variant,
                primary: scheme.primary,
                mark: scheme.on_primary,
                unchecked_fill: (theme.brightness == Brightness::Dark)
                    .then(|| style::scale_alpha(scheme.outline_variant, DARK_FILL_ALPHA)),
            }
        }
        None => CheckboxColors {
            border: FALLBACK_INPUT,
            primary: FALLBACK_PRIMARY,
            mark: FALLBACK_PRIMARY_FOREGROUND,
            unchecked_fill: None,
        },
    }
}

/// The lucide `CheckIcon` path (`M20 6 9 17l-5-5`), scaled from its 24-unit
/// viewBox to a `size`-square box and translated to `origin`.
fn check_path(origin: Point, size: f64) -> BezPath {
    let s = size / ICON_VIEWBOX;
    let at = |x: f64, y: f64| Point::new(origin.x + x * s, origin.y + y * s);
    let mut path = BezPath::new();
    path.move_to(at(4.0, 12.0));
    path.line_to(at(9.0, 17.0));
    path.line_to(at(20.0, 6.0));
    path
}

/// The lucide `MinusIcon` path (`M5 12h14`), scaled from its 24-unit viewBox to
/// a `size`-square box and translated to `origin` — the indeterminate mark.
fn minus_path(origin: Point, size: f64) -> BezPath {
    let s = size / ICON_VIEWBOX;
    let at = |x: f64, y: f64| Point::new(origin.x + x * s, origin.y + y * s);
    let mut path = BezPath::new();
    path.move_to(at(5.0, 12.0));
    path.line_to(at(19.0, 12.0));
    path
}

/// Whether `key` activates a control: `Space` (WAI-ARIA's checkbox key, arriving
/// as typed text since there is no `NamedKey::Space`) or `Enter`.
///
/// Upstream leaves `Enter` to a form submit; frust has no form to submit into, so
/// the catalog accepts both rather than leaving `Enter` inert.
fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
    }
}

impl<State: 'static> View<State> for CheckboxView<State> {
    type Element = CheckboxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CheckboxWidget {
        CheckboxWidget {
            checked: self.checked,
            indeterminate: self.indeterminate,
            disabled: self.disabled,
            label: self.label.clone(),
            captured: false,
            on_checked_change: erase_callback_arg(&self.on_checked_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CheckboxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_checked_change = erase_callback_arg(&self.on_checked_change);
        let mut flags = ChangeFlags::NONE;
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the confirmed value.
            element.checked = self.checked;
            flags |= ChangeFlags::PAINT;
        }
        if prev.indeterminate != self.indeterminate {
            // Same contract as `checked`: the app owns the mixed state too.
            element.indeterminate = self.indeterminate;
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`CheckboxView`].
pub struct CheckboxWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    /// The app-confirmed mixed state, which outranks `checked` everywhere it is
    /// painted or announced (source of truth; adopted on `rebuild`).
    indeterminate: bool,
    disabled: bool,
    label: Option<String>,
    /// Armed by a `Down` inside (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`. Nothing paints differently while armed — the source
    /// authors no `:active` rule.
    captured: bool,
    on_checked_change: frust::authoring::ErasedArgCallback<bool>,
}

impl CheckboxWidget {
    /// The cursor this control asks for in its current state.
    fn cursor(&self) -> CursorIcon {
        if self.disabled {
            style::DISABLED_CURSOR
        } else {
            style::ACTIVE_CURSOR
        }
    }

    /// Report the requested value without touching any of this widget's own
    /// state: `!checked` normally, and `true` from the mixed state (Radix
    /// resolves an indeterminate box upward — see the [module docs](self)).
    fn request_toggle(&mut self, ctx: &mut EventCtx) {
        let next = if self.indeterminate {
            true
        } else {
            !self.checked
        };
        (self.on_checked_change)(ctx, next);
    }

    /// Whether the box paints its filled treatment: checked *or* mixed, both of
    /// which carry a mark on the `primary` fill.
    fn filled(&self) -> bool {
        self.checked || self.indeterminate
    }
}

impl Widget for CheckboxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let focused = ctx.has_focus();
        let origin = ctx.origin();
        let size = Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE);
        let tint = |color: Color| style::disabled_tint(color, self.disabled);

        style::draw_shadow(
            scene,
            origin,
            size,
            CHECKBOX_RADIUS,
            style::SHADOW_XS,
            theme,
        );

        let fill = if self.filled() {
            Some(colors.primary)
        } else {
            colors.unchecked_fill
        };
        if let Some(fill) = fill {
            scene.fill_rounded_rect(origin, size, CHECKBOX_RADIUS, tint(fill));
        }

        // The border is the checked/unchecked token, overridden by the ring
        // color while focused (`focus-visible:border-ring`).
        let resting_border = if self.filled() {
            colors.primary
        } else {
            colors.border
        };
        let border = style::focus_border(resting_border, focused, theme);
        let inset = style::BORDER_WIDTH / 2.0;
        let outline = frust::authoring::RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
            (CHECKBOX_RADIUS - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &frust::authoring::Shape::to_path(&outline, PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(tint(border)),
        );

        if self.filled() {
            let inset = (CHECKBOX_SIZE - CHECK_SIZE) / 2.0;
            let at = Point::new(inset, inset);
            // The mixed state's minus outranks the check, the way Radix's third
            // value outranks the boolean.
            let path = if self.indeterminate {
                minus_path(at, CHECK_SIZE)
            } else {
                check_path(at, CHECK_SIZE)
            };
            scene.stroke_path(
                origin,
                &path,
                CHECK_STROKE_VIEWBOX * CHECK_SIZE / ICON_VIEWBOX,
                &Brush::Solid(tint(colors.mark)),
            );
        }

        if focused {
            style::draw_focus_ring(
                scene,
                origin,
                size,
                CHECKBOX_RADIUS,
                style::ring_color(None, theme),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                self.request_toggle(ctx);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if self.disabled || !inside(p.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        ctx.capture_pointer();
                        // The focus ring is the only thing a press changes here,
                        // and it is what earns the redraw.
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            // The hover/cursor pass: claim on every qualifying
                            // move (the claim is per-pass, never sticky) and
                            // re-ask for the cursor, which is stateless too. A
                            // disabled control still answers with its own shape.
                            if inside(p.position, size) {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag that
                        // wandered outside the box.
                        ctx.set_cursor(self.cursor());
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        if inside(p.position, size) {
                            self.request_toggle(ctx);
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // A Cancel arm clears internal flags only — never state,
                        // never the callback.
                        self.captured = false;
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::CheckBox, |node| {
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
            node.set_toggled(if self.indeterminate {
                Toggled::Mixed
            } else {
                Toggled::from(self.checked)
            });
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
    use frust::authoring::Shape;
    use frust::authoring::{
        EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::{Brightness, FrameTime};
    use std::any::Any;

    /// Records the paint calls this widget emits: rounded-rect fills, stroked
    /// paths (bounding box + width + color) and shadows.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
    }

    #[derive(Default)]
    struct Toggles {
        last: Option<bool>,
        count: u32,
    }

    fn view(checked: bool, disabled: bool) -> CheckboxView<Toggles> {
        checkbox::<Toggles, _>(checked, |s: &mut Toggles, v: bool| {
            s.last = Some(v);
            s.count += 1;
        })
        .disabled(disabled)
        .label("terms")
    }

    fn widget(checked: bool, disabled: bool) -> CheckboxWidget {
        let mut counter = 0u64;
        View::<Toggles>::build(&view(checked, disabled), &mut BuildCtx::new(&mut counter))
    }

    fn mixed_widget(checked: bool) -> CheckboxWidget {
        let mut counter = 0u64;
        View::<Toggles>::build(
            &view(checked, false).indeterminate(true),
            &mut BuildCtx::new(&mut counter),
        )
    }

    fn paint(w: &mut CheckboxWidget, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let size = Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE);
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn space() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(" ".into()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(w: &mut CheckboxWidget, state: &mut Toggles, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(
            state_any,
            Point::ZERO,
            Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE),
        );
        w.event(&mut ctx, event)
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn unchecked_light_paints_a_bordered_box_with_no_fill() {
        let mut w = widget(false, false);
        let rec = paint(&mut w, None);
        assert!(
            rec.rrects.is_empty(),
            "light mode authors no `bg-*` on an unchecked box"
        );
        assert_eq!(rec.strokes.len(), 1, "the border, and nothing else");
        let (_, width, color) = rec.strokes[0];
        assert_eq!(width, style::BORDER_WIDTH);
        assert_eq!(color, FALLBACK_INPUT);
        // `shadow-xs` rides both states.
        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].3, style::SHADOW_XS.std_dev);
    }

    #[test]
    fn checked_paint_fills_primary_and_strokes_the_check_in_primary_foreground() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let mut w = widget(true, false);
        let rec = paint(&mut w, Some(&theme));

        assert_eq!(rec.rrects.len(), 1);
        let (_, size, radius, fill) = rec.rrects[0];
        assert_eq!(size, Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE));
        assert_eq!(radius, CHECKBOX_RADIUS);
        assert_eq!(fill, scheme.primary);

        // Border (primary) then the check mark (primary-foreground).
        assert_eq!(rec.strokes.len(), 2);
        assert_eq!(rec.strokes[0].2, scheme.primary);
        let (bbox, width, mark) = rec.strokes[1];
        assert_eq!(mark, scheme.on_primary);
        assert!((width - CHECK_STROKE_VIEWBOX * CHECK_SIZE / ICON_VIEWBOX).abs() < 1e-9);
        // The 14px icon sits centred in the 16px box.
        let inset = (CHECKBOX_SIZE - CHECK_SIZE) / 2.0;
        assert!(bbox.x0 >= inset - 1e-9 && bbox.x1 <= CHECKBOX_SIZE - inset + 1e-9);
    }

    #[test]
    fn dark_mode_unchecked_paints_the_input_wash() {
        let theme = crate::theme().with_brightness(Brightness::Dark);
        let mut w = widget(false, false);
        let rec = paint(&mut w, Some(&theme));
        assert_eq!(rec.rrects.len(), 1, "`dark:bg-input/30`");
        let fill = rec.rrects[0].3;
        let input = theme.scheme().outline_variant;
        assert!((fill.components[3] - input.components[3] * DARK_FILL_ALPHA).abs() < 1e-6);
    }

    #[test]
    fn disabled_paint_halves_every_painted_alpha() {
        let theme = crate::theme();
        let mut enabled = widget(true, false);
        let mut disabled = widget(true, true);
        let on = paint(&mut enabled, Some(&theme));
        let off = paint(&mut disabled, Some(&theme));
        assert_eq!(
            off.rrects[0].3.components[3],
            on.rrects[0].3.components[3] * style::DISABLED_OPACITY
        );
        assert_eq!(
            off.strokes[1].2.components[3],
            on.strokes[1].2.components[3] * style::DISABLED_OPACITY
        );
    }

    #[test]
    fn an_indeterminate_box_fills_primary_and_strokes_a_minus() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let mut w = mixed_widget(false);
        let rec = paint(&mut w, Some(&theme));

        assert_eq!(
            rec.rrects.len(),
            1,
            "the mixed state fills like a checked one"
        );
        assert_eq!(rec.rrects[0].3, scheme.primary);
        assert_eq!(rec.strokes.len(), 2, "border + mark");
        assert_eq!(rec.strokes[0].2, scheme.primary, "`border-primary`");

        // The minus is a flat horizontal run, unlike the check's two segments.
        let (bbox, width, mark) = rec.strokes[1];
        assert_eq!(mark, scheme.on_primary);
        assert!(bbox.height() < 1e-9, "`M5 12h14` has no vertical extent");
        assert!((width - CHECK_STROKE_VIEWBOX * CHECK_SIZE / ICON_VIEWBOX).abs() < 1e-9);
        // ...and it sits centred in the 16px box, 5/24ths in from the icon edge.
        let inset = (CHECKBOX_SIZE - CHECK_SIZE) / 2.0;
        assert!((bbox.y0 - CHECKBOX_SIZE / 2.0).abs() < 1e-9);
        assert!((bbox.x0 - (inset + 5.0 * CHECK_SIZE / ICON_VIEWBOX)).abs() < 1e-9);
    }

    #[test]
    fn the_mixed_state_outranks_checked_in_paint() {
        let theme = crate::theme();
        let mut checked_and_mixed = mixed_widget(true);
        let rec = paint(&mut checked_and_mixed, Some(&theme));
        assert!(
            rec.strokes[1].0.height() < 1e-9,
            "a checked box that is also mixed still paints the minus"
        );
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_the_requested_value_without_self_mutating() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 8.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 8.0, 8.0));
        assert_eq!(state.last, Some(true));
        assert_eq!(state.count, 1);
        assert!(!w.checked, "the app owns `checked`, not the widget");
        assert!(!w.captured);

        let mut on = widget(true, false);
        dispatch(&mut on, &mut state, &pointer(PointerPhase::Down, 8.0, 8.0));
        dispatch(&mut on, &mut state, &pointer(PointerPhase::Up, 8.0, 8.0));
        assert_eq!(state.last, Some(false));
        assert!(on.checked);
    }

    /// Radix resolves a mixed box **upward**: activating it asks for `true`,
    /// never `!checked`, whichever boolean the mixed value sits on top of.
    #[test]
    fn activating_an_indeterminate_box_asks_for_true() {
        for checked in [false, true] {
            let mut w = mixed_widget(checked);
            let mut state = Toggles::default();
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 8.0));
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 8.0, 8.0));
            assert_eq!(state.last, Some(true), "checked = {checked}");
            assert!(w.indeterminate, "the app owns the mixed state too");

            dispatch(&mut w, &mut state, &space());
            assert_eq!(state.last, Some(true), "the key path agrees");
            assert_eq!(state.count, 2);
        }
    }

    #[test]
    fn up_outside_and_cancel_never_fire() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 8.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 80.0, 8.0));
        assert_eq!(state.count, 0);

        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 8.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Cancel, 8.0, 8.0));
        assert_eq!(state.count, 0);
        assert!(!w.captured, "Cancel disarms");
        // A stray `Up` after the cancel is not a press.
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 8.0, 8.0));
        assert_eq!(state.count, 0);
    }

    #[test]
    fn space_and_enter_activate_a_focused_checkbox() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        assert_eq!(dispatch(&mut w, &mut state, &space()), EventResult::Handled);
        assert_eq!(state.last, Some(true));

        let enter = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        dispatch(&mut w, &mut state, &enter);
        assert_eq!(state.count, 2);

        // An unrelated key is not an activation.
        let tab = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(dispatch(&mut w, &mut state, &tab), EventResult::Ignored);
        assert_eq!(state.count, 2);
    }

    #[test]
    fn a_disabled_checkbox_is_inert_to_pointer_and_key() {
        let mut w = widget(false, true);
        let mut state = Toggles::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 8.0)),
            EventResult::Ignored
        );
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 8.0, 8.0));
        assert_eq!(dispatch(&mut w, &mut state, &space()), EventResult::Ignored);
        assert_eq!(state.count, 0);
    }

    #[test]
    fn a_hover_move_claims_without_pressing_or_repainting() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(
            state_any,
            Point::ZERO,
            Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE),
        );
        let result = w.event(&mut ctx, &pointer(PointerPhase::Move, 8.0, 8.0));
        assert_eq!(result, EventResult::Ignored, "watching is not consuming");
        assert!(!w.captured);
        assert!(
            !ctx.needs_redraw(),
            "no hover chrome upstream, so no frame is owed"
        );
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_disarms_on_disable() {
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        w.captured = true;

        let next = view(true, true);
        let flags =
            View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.checked);
        assert!(w.disabled);
        assert!(!w.captured, "disabling clears an armed press");
        assert!(flags.needs_paint());

        // The mixed state reconciles the same way, in both directions.
        let mixed = view(true, true).indeterminate(true);
        let flags =
            View::<Toggles>::rebuild(&mixed, &next, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.indeterminate);
        assert!(flags.needs_paint());
        let flags =
            View::<Toggles>::rebuild(&next, &mixed, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(!w.indeterminate);
        assert!(flags.needs_paint());
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    /// One checkbox under a real `RenderRoot` — the only harness that can
    /// exercise focus, hover and the cursor, since all three are recorded by the
    /// root's event pass and read back through the paint context / `cursor()`.
    struct Harness {
        root: frust_core::RenderRoot<Toggles, CheckboxView<Toggles>>,
        state: Toggles,
        disabled: bool,
        indeterminate: bool,
    }

    impl Harness {
        fn new(disabled: bool) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Toggles::default(),
                disabled,
                indeterminate: false,
            };
            h.root.set_theme(Box::new(crate::theme()));
            h.rebuild();
            h
        }

        fn rebuild(&mut self) {
            let disabled = self.disabled;
            let indeterminate = self.indeterminate;
            let mut logic = move |_s: &mut Toggles| {
                checkbox::<Toggles, _>(false, |s: &mut Toggles, v: bool| {
                    s.last = Some(v);
                    s.count += 1;
                })
                .disabled(disabled)
                .indeterminate(indeterminate)
                .label("terms")
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root.layout(Size::new(200.0, 200.0));
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn a_press_claims_focus_and_the_next_paint_shows_the_ring() {
        let mut h = Harness::new(false);
        let resting = h.paint();
        assert_eq!(resting.strokes.len(), 1, "border only at rest");

        h.dispatch(&pointer(PointerPhase::Down, 8.0, 8.0));
        assert!(h.root.is_focus_active());
        let focused = h.paint();
        assert_eq!(focused.strokes.len(), 2, "border + focus ring");
        let ring = style::ring_color(None, Some(&crate::theme()));
        // The border swapped to the ring color...
        assert_eq!(focused.strokes[0].2, ring);
        // ...and the 3px ring sits outside the box at ring/50.
        let (bbox, width, color) = focused.strokes[1];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color.components[3], style::FOCUS_RING_OPACITY);
        assert!(bbox.x0 < 0.0 && bbox.x1 > CHECKBOX_SIZE);
    }

    #[test]
    fn a_move_resolves_the_pointer_cursor_and_not_allowed_when_disabled() {
        let mut h = Harness::new(false);
        h.dispatch(&pointer(PointerPhase::Move, 8.0, 8.0));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        let mut disabled = Harness::new(true);
        disabled.dispatch(&pointer(PointerPhase::Move, 8.0, 8.0));
        assert_eq!(disabled.root.cursor(), style::DISABLED_CURSOR);

        // Off the box, nothing asks for a shape.
        let mut away = Harness::new(false);
        away.dispatch(&pointer(PointerPhase::Move, 150.0, 150.0));
        assert_eq!(away.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn semantics_reports_a_checkbox_node_with_its_toggle_state() {
        let mut h = Harness::new(false);
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::CheckBox)
            .expect("a Role::CheckBox node");
        assert_eq!(node.label(), Some("terms"));
        assert_eq!(node.toggled(), Some(Toggled::False));
        assert!(node.supports_action(Action::Click));

        h.disabled = true;
        h.rebuild();
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::CheckBox)
            .expect("a Role::CheckBox node");
        assert!(node.is_disabled());
        assert!(!node.supports_action(Action::Click));
    }

    #[test]
    fn semantics_reports_the_mixed_state_as_toggled_mixed() {
        let mut h = Harness::new(false);
        h.indeterminate = true;
        h.rebuild();
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::CheckBox)
            .expect("a Role::CheckBox node");
        assert_eq!(node.toggled(), Some(Toggled::Mixed), "aria-checked=mixed");
        assert!(node.supports_action(Action::Click));
    }
}
