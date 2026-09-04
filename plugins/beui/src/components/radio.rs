//! Ports beUI's **Radio Group** item — the ring whose dot pops in on the
//! catalog's layout spring.
//!
//! Source: `components/motion/radio.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `radio`: *"Single-select choice control with a gliding layoutId
//! indicator dot and spring press feedback."*
//!
//! | class / prop | here |
//! |---|---|
//! | `h-5 w-5 rounded-full border-2` | [`RADIO_SIZE`], a pill, [`RADIO_BORDER_WIDTH`] |
//! | unselected `border-muted-foreground/50` | `on_surface_variant` at [`BORDER_IDLE_ALPHA`] |
//! | `hover:border-muted-foreground` | the same token at full alpha |
//! | selected `border-primary` | `primary` |
//! | dot `absolute inset-1 rounded-full bg-primary` | [`RADIO_DOT_INSET`] → [`RADIO_DOT_SIZE`] |
//! | `transition-colors duration-200` | [`switch::TRACK_COLOR_RAMP`] |
//! | dot `layoutId` + `SPRING_LAYOUT` | [`SPRING_LAYOUT`] driving the dot's pop |
//! | `whileTap={{ scale: 0.92 }}` + `SPRING_PRESS` | [`RADIO_PRESS_SCALE`] |
//! | `focus-visible:ring-2 ring-ring ring-offset-2` | [`switch::draw_focus_ring`] at [`RADIO_RING_OFFSET`] |
//! | `disabled:opacity-60 disabled:cursor-not-allowed` | [`DISABLED_OPACITY`] + [`style::DISABLED_CURSOR`] |
//!
//! # The group is composition, not a widget
//!
//! Upstream's `<RadioGroup>` is a `<div role="radiogroup">` plus a React
//! context carrying the selected value, a setter and a shared `layoutId`. The
//! value plumbing has no counterpart here — a frust app already owns its state
//! — so the port is the **item**: build a group as a `Column`/`Row` of
//! [`radio`] views, each told whether it is the selected one and each reporting
//! its own selection. That keeps the app the single source of truth, which is
//! this catalog's rule for every control.
//!
//! # Degradation: the dot pops, it does not glide between items
//!
//! The shared `layoutId` is what makes upstream's dot *travel* from the
//! previously selected item to the newly selected one, because Motion measures
//! both DOM nodes and interpolates between them. frust has no shared-layout
//! registry spanning independent sibling widgets, and one item cannot see
//! another's box, so the ported dot **scales in and out in place** on the same
//! [`SPRING_LAYOUT`] instead of crossing the gap. The spring, the size and the
//! colour are upstream's; the travel is not reproduced.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, Affine, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    RoundedRect, SemanticsCtx, Shape, Size, View, Widget, erase_callback,
};

use super::switch;
use crate::motion::Ramp;
use crate::press::{
    Lane, draw_focus_ring, inside_inclusive as inside, is_activation_key, lerp_color, press_scale,
    presses,
};
use crate::style;
use crate::tokens::BeuiTokens;
use crate::tokens::motion::{SPRING_LAYOUT, SPRING_PRESS};

/// Ring edge, in logical px (`h-5 w-5`).
pub const RADIO_SIZE: f64 = 20.0;

/// Border width, in logical px (`border-2`).
pub const RADIO_BORDER_WIDTH: f64 = 2.0;

/// How far the dot is inset from the ring's box, in logical px (`inset-1`).
pub const RADIO_DOT_INSET: f64 = 4.0;

/// Dot diameter, in logical px — the ring less its two insets.
pub const RADIO_DOT_SIZE: f64 = RADIO_SIZE - 2.0 * RADIO_DOT_INSET;

/// The scale a pressed ring shrinks to (`whileTap={{ scale: 0.92 }}`).
pub const RADIO_PRESS_SCALE: f64 = 0.92;

/// Gap between the ring and its focus ring, in logical px (`ring-offset-2`).
pub const RADIO_RING_OFFSET: f64 = 2.0;

/// Alpha of the idle unselected border (`border-muted-foreground/50`); a hover
/// takes it to full (`hover:border-muted-foreground`).
pub const BORDER_IDLE_ALPHA: f32 = 0.5;

/// Opacity of a disabled radio: `disabled:opacity-60`.
const DISABLED_OPACITY: f32 = 0.6;

/// Unthemed fallback ink — the light table's `--primary`.
const FALLBACK_PRIMARY: Color = crate::BEUI_LIGHT.primary;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = crate::BEUI_LIGHT.muted_foreground;

/// A view-held, typed select callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI radio item. See the [module docs](self).
pub struct RadioView<State: 'static> {
    selected: bool,
    disabled: bool,
    label: Option<String>,
    on_select: OnSelect<State>,
}

/// Create a radio item reflecting `selected` that reports `on_select(state)` on
/// a release inside its bounds — a **controlled** component (see the
/// [module docs](self)).
///
/// A re-selection of the already-selected item still reports, exactly as
/// upstream's `onClick={() => setValue(value)}` does; a group that treats that
/// as a no-op does so in its own handler.
pub fn radio<State: 'static, F: Fn(&mut State) + 'static>(
    selected: bool,
    on_select: F,
) -> RadioView<State> {
    RadioView {
        selected,
        disabled: false,
        label: None,
        on_select: Rc::new(on_select),
    }
}

impl<State: 'static> RadioView<State> {
    /// Disable the item: 60% opacity, inert, not-allowed cursor.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the item for assistive tech (the ring paints no text of its own —
    /// upstream renders its `label` prop as a sibling `<span>`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The resolved radio palette.
struct RadioColors {
    /// `border-muted-foreground/50`, or full alpha while hovered.
    border_idle: Color,
    /// `border-primary` and the dot's `bg-primary`.
    primary: Color,
    /// `--ring`, the focus ring.
    ring: Color,
}

/// Resolve the palette, falling back to the vendored light table with no theme
/// threaded.
fn resolve_colors(theme: Option<&Theme>, hovered: bool) -> RadioColors {
    let alpha = if hovered { 1.0 } else { BORDER_IDLE_ALPHA };
    let ring = BeuiTokens::resolve_ring(None, theme);
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            RadioColors {
                border_idle: style::with_alpha(scheme.on_surface_variant, alpha),
                primary: scheme.primary,
                ring,
            }
        }
        None => RadioColors {
            border_idle: style::with_alpha(FALLBACK_MUTED_FOREGROUND, alpha),
            primary: FALLBACK_PRIMARY,
            ring,
        },
    }
}

impl<State: 'static> View<State> for RadioView<State> {
    type Element = RadioWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RadioWidget {
        let rest = if self.selected { 1.0 } else { 0.0 };
        RadioWidget {
            selected: self.selected,
            disabled: self.disabled,
            label: self.label.clone(),
            blend: Lane::at_rest(switch::TRACK_COLOR_RAMP, rest),
            dot: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), rest),
            press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            hovered: false,
            captured: false,
            on_select: erase_callback(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RadioWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_select = erase_callback(&self.on_select);
        let mut flags = ChangeFlags::NONE;
        if prev.selected != self.selected {
            // The app is the source of truth: adopt the confirmed value and
            // animate toward it.
            element.selected = self.selected;
            let target = if self.selected { 1.0 } else { 0.0 };
            element.blend.retarget(target);
            element.dot.retarget(target);
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = false;
                element.hovered = false;
                element.press.retarget(0.0);
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

/// The retained widget for a [`RadioView`].
pub struct RadioWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    selected: bool,
    disabled: bool,
    label: Option<String>,
    /// The border crossfade between the idle and selected treatments.
    blend: Lane,
    /// The dot's scale, `0.0` gone .. `1.0` full — on [`SPRING_LAYOUT`], which
    /// is the spring upstream's shared-layout dot travels on.
    dot: Lane,
    /// The press shrink, `0.0` .. `1.0`.
    press: Lane,
    /// The latched hover, self-corrected from `PaintCtx::is_hovered` each paint.
    hovered: bool,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`.
    captured: bool,
    on_select: frust::authoring::ErasedCallback,
}

impl RadioWidget {
    /// The cursor this control asks for in its current state.
    fn cursor(&self) -> CursorIcon {
        if self.disabled {
            style::DISABLED_CURSOR
        } else {
            style::ACTIVE_CURSOR
        }
    }
}

impl Widget for RadioWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(RADIO_SIZE, RADIO_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() || self.disabled {
            self.hovered = false;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme, self.hovered && !self.selected);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();
        let now = ctx.frame_time();

        let mut owes_frame = false;
        if reduce {
            self.blend.snap();
            self.dot.snap();
            self.press.snap();
        } else {
            owes_frame |= self.blend.advance(now);
            owes_frame |= self.dot.advance(now);
            owes_frame |= self.press.advance(now);
        }

        let ring = Size::new(RADIO_SIZE, RADIO_SIZE);
        let radius = style::resolve_radius(style::RADIUS_CONTROL, ring.width, ring.height);
        let origin = ctx.origin();
        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);
        let blend = self.blend.value().clamp(0.0, 1.0);

        // `whileTap` scales the whole control about its own centre.
        let scale = press_scale(RADIO_PRESS_SCALE, self.press.value());
        let centre = origin + (ring.to_vec2() / 2.0);
        scene.push_transform(
            Affine::translate(centre.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-centre.to_vec2()),
        );

        // `border-2` is inside the box (`box-sizing: border-box`), so the
        // stroke is centred half a border in from the edge.
        let inset = RADIO_BORDER_WIDTH / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, ring).inset(-inset),
            (radius - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&outline, style::PATH_TOLERANCE),
            RADIO_BORDER_WIDTH,
            &Brush::Solid(tint(lerp_color(colors.border_idle, colors.primary, blend))),
        );

        // The dot: `inset-1` of the ring, scaled about the ring's centre by the
        // layout spring. Clamped at the low end so an over-damped spring cannot
        // paint a negative box, and left unclamped above so an under-damped one
        // would still be free to overshoot.
        let dot = self.dot.value().max(0.0);
        if dot > 0.0 {
            let size = RADIO_DOT_SIZE * dot;
            let dot_origin = Point::new(centre.x - size / 2.0, centre.y - size / 2.0);
            scene.fill_rounded_rect(
                dot_origin,
                Size::new(size, size),
                size / 2.0,
                tint(colors.primary),
            );
        }

        scene.pop_transform();

        if focused {
            draw_focus_ring(
                scene,
                origin,
                ring,
                radius,
                RADIO_RING_OFFSET,
                tint(colors.ring),
            );
        }

        // Paint-only animation (the ring never resizes), so a bare frame
        // request is the right one — never `request_layout`.
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                (self.on_select)(ctx);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if self.disabled || !presses(p) || !inside(p.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        self.press.retarget(1.0);
                        ctx.capture_pointer();
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            let over = inside(p.position, size);
                            if over {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            let hovered = over && !self.disabled;
                            if self.hovered != hovered {
                                self.hovered = hovered;
                                ctx.request_redraw();
                            }
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag outside
                        // the ring, and let the press scale go off it.
                        ctx.set_cursor(self.cursor());
                        let want = if inside(p.position, size) { 1.0 } else { 0.0 };
                        if self.press.target() != want {
                            self.press.retarget(want);
                            ctx.request_redraw();
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        self.press.retarget(0.0);
                        if inside(p.position, size) {
                            (self.on_select)(ctx);
                        }
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // Internal state only — never the callback.
                        self.captured = false;
                        self.press.retarget(0.0);
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::RadioButton, |node| {
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
            node.set_selected(self.selected);
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
    use frust::FrameTime;
    use frust::authoring::{
        BezPath, EventOutcome, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
        SemanticsUpdate,
    };
    use std::any::Any;

    /// Records the fills, stroked paths and transform pushes this widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        transforms: Vec<Affine>,
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
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
    }

    impl Recorder {
        /// The border stroke of the pass.
        fn border(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w == RADIO_BORDER_WIDTH)
                .expect("the 2px border was stroked")
        }

        /// The dot, if the pass painted one.
        fn dot(&self) -> Option<(Point, Size, f64, Color)> {
            self.rrects.first().copied()
        }
    }

    #[derive(Default)]
    struct Picks {
        count: u32,
    }

    const RING: Size = Size::new(RADIO_SIZE, RADIO_SIZE);

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn view(selected: bool, disabled: bool) -> RadioView<Picks> {
        radio::<Picks, _>(selected, |s: &mut Picks| s.count += 1)
            .disabled(disabled)
            .label("weekly")
    }

    fn widget(selected: bool, disabled: bool) -> RadioWidget {
        let mut counter = 0u64;
        View::<Picks>::build(&view(selected, disabled), &mut BuildCtx::new(&mut counter))
    }

    fn paint_at(w: &mut RadioWidget, theme: Option<&Theme>, millis: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, RING, ft_ms(millis));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn paint(w: &mut RadioWidget, theme: Option<&Theme>) -> Recorder {
        paint_at(w, theme, 0.0).0
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut RadioWidget, state: &mut Picks, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, RING);
        w.event(&mut ctx, event)
    }

    // ---- Metrics / paint --------------------------------------------------

    #[test]
    fn the_ring_and_its_dot_carry_the_authored_metrics() {
        assert_eq!(RADIO_SIZE, 20.0, "h-5 w-5");
        assert_eq!(RADIO_BORDER_WIDTH, 2.0, "border-2");
        assert_eq!(RADIO_DOT_INSET, 4.0, "inset-1");
        assert_eq!(RADIO_DOT_SIZE, 12.0);
    }

    #[test]
    fn an_unselected_ring_paints_a_half_alpha_border_and_no_dot() {
        let mut w = widget(false, false);
        let rec = paint(&mut w, None);
        assert!(rec.dot().is_none(), "no indicator until it is selected");
        let (_, width, color) = rec.border();
        assert_eq!(width, RADIO_BORDER_WIDTH);
        assert_eq!(color.components[3], BORDER_IDLE_ALPHA);
    }

    #[test]
    fn a_selected_ring_paints_a_primary_border_and_a_full_size_dot() {
        let theme = crate::theme();
        let mut w = widget(true, false);
        let (rec, owes) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes, "an item that mounts selected rests there");
        assert_eq!(rec.border().2, theme.scheme().primary, "border-primary");

        let (dot_origin, size, radius, fill) = rec.dot().expect("the dot is filled");
        assert_eq!(size, Size::new(RADIO_DOT_SIZE, RADIO_DOT_SIZE));
        assert_eq!(radius, RADIO_DOT_SIZE / 2.0, "a circle");
        assert_eq!(fill, theme.scheme().primary, "bg-primary");
        assert_eq!(dot_origin, Point::new(RADIO_DOT_INSET, RADIO_DOT_INSET));
    }

    #[test]
    fn disabled_paint_dims_every_painted_alpha_to_sixty_percent() {
        let theme = crate::theme();
        let mut enabled = widget(true, false);
        let mut disabled = widget(true, true);
        let on = paint(&mut enabled, Some(&theme));
        let off = paint(&mut disabled, Some(&theme));
        assert!(
            (off.border().2.components[3] - on.border().2.components[3] * DISABLED_OPACITY).abs()
                < 1e-6
        );
        assert!(
            (off.dot().expect("dot").3.components[3]
                - on.dot().expect("dot").3.components[3] * DISABLED_OPACITY)
                .abs()
                < 1e-6
        );
    }

    // ---- Motion -----------------------------------------------------------

    #[test]
    fn selecting_pops_the_dot_open_on_the_layout_spring() {
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Picks>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, false);
        View::<Picks>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (start, owes) = paint_at(&mut w, None, 0.0);
        assert!(owes, "an in-flight lane owes the next frame");
        assert!(start.dot().is_none(), "the dot starts at zero size");

        let (mid, owes) = paint_at(&mut w, None, 60.0);
        assert!(owes);
        let size = mid.dot().expect("a growing dot").1;
        assert!(
            size.width > 0.0 && size.width < RADIO_DOT_SIZE,
            "mid-pop: {size:?}"
        );
        // ...and it stays centred in the ring as it grows.
        let origin = mid.dot().expect("dot").0;
        assert!((origin.x + size.width / 2.0 - RADIO_SIZE / 2.0).abs() < 1e-9);

        let (end, owes) = paint_at(&mut w, None, 3_000.0);
        assert!(!owes, "a settled lane asks for nothing");
        assert_eq!(
            end.dot().expect("dot").1,
            Size::new(RADIO_DOT_SIZE, RADIO_DOT_SIZE)
        );
        assert_eq!(end.border().2, FALLBACK_PRIMARY, "landed on-token");
    }

    #[test]
    fn deselecting_collapses_the_dot_back_to_nothing() {
        let mut counter = 0u64;
        let prev = view(true, false);
        let mut w = View::<Picks>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(false, false);
        View::<Picks>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        paint_at(&mut w, None, 0.0);
        let mid = paint_at(&mut w, None, 60.0).0;
        let size = mid.dot().expect("a shrinking dot").1;
        assert!(size.width > 0.0 && size.width < RADIO_DOT_SIZE);

        let (end, owes) = paint_at(&mut w, None, 3_000.0);
        assert!(!owes);
        assert!(end.dot().is_none(), "gone");
    }

    #[test]
    fn reduce_motion_snaps_the_dot_open_and_owes_no_frame() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Picks>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, false);
        View::<Picks>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (rec, owes) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes, "reduce_motion asks for no animation frame");
        assert_eq!(
            rec.dot().expect("dot").1,
            Size::new(RADIO_DOT_SIZE, RADIO_DOT_SIZE)
        );
        assert_eq!(rec.border().2, theme.scheme().primary);
    }

    #[test]
    fn a_held_ring_shrinks_under_the_press_spring() {
        let mut w = widget(false, false);
        let mut state = Picks::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        paint_at(&mut w, None, 0.0);
        let (rec, owes) = paint_at(&mut w, None, 30.0);
        assert!(owes);
        let scale = rec.transforms[0].as_coeffs()[0];
        assert!(
            (RADIO_PRESS_SCALE..1.0).contains(&scale),
            "press scale {scale}"
        );
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_without_self_selecting() {
        let mut w = widget(false, false);
        let mut state = Picks::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.count, 1);
        assert!(!w.selected, "the app owns `selected`");
        assert!(!w.captured);
    }

    #[test]
    fn re_selecting_the_selected_item_still_reports() {
        let mut w = widget(true, false);
        let mut state = Picks::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            state.count, 1,
            "`setValue(value)` is unconditional upstream"
        );
    }

    #[test]
    fn up_outside_cancel_and_a_secondary_press_never_fire() {
        let mut w = widget(false, false);
        let mut state = Picks::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 90.0, 10.0));
        assert_eq!(state.count, 0);

        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Cancel, 10.0, 10.0),
        );
        assert_eq!(state.count, 0);
        assert!(!w.captured);

        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(10.0, 10.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &secondary),
            EventResult::Ignored
        );
        assert!(!w.captured);
    }

    #[test]
    fn space_and_enter_select_and_a_disabled_item_is_inert() {
        let mut w = widget(false, false);
        let mut state = Picks::default();
        let space = InputEvent::Key(KeyEvent {
            key: Key::Character(" ".into()),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(dispatch(&mut w, &mut state, &space), EventResult::Handled);
        let enter = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        dispatch(&mut w, &mut state, &enter);
        assert_eq!(state.count, 2);

        let mut disabled = widget(false, true);
        assert_eq!(
            dispatch(&mut disabled, &mut state, &space),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(
                &mut disabled,
                &mut state,
                &pointer(PointerPhase::Down, 10.0, 10.0)
            ),
            EventResult::Ignored
        );
        assert_eq!(state.count, 2, "only the enabled item reported");
    }

    #[test]
    fn a_hover_brightens_the_unselected_border_and_paint_corrects_the_latch() {
        let mut w = widget(false, false);
        let mut state = Picks::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, RING);
        w.event(&mut ctx, &pointer(PointerPhase::Move, 10.0, 10.0));
        assert!(w.hovered);
        assert!(ctx.needs_redraw());

        let rec = paint(&mut w, None);
        assert_eq!(rec.border().2.components[3], BORDER_IDLE_ALPHA);
        assert!(!w.hovered);
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_disarms_on_disable() {
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Picks>::build(&prev, &mut BuildCtx::new(&mut counter));
        w.captured = true;
        let next = view(true, true);
        let flags = View::<Picks>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.selected);
        assert!(w.disabled);
        assert!(!w.captured, "disabling clears an armed press");
        assert!(flags.needs_paint());
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    /// One radio item under a real `RenderRoot` — the only harness that can
    /// exercise focus and the cursor.
    struct Harness {
        root: frust_core::RenderRoot<Picks, RadioView<Picks>>,
        state: Picks,
    }

    impl Harness {
        fn new(disabled: bool) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Picks::default(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Picks| view(false, disabled);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout(Size::new(200.0, 200.0));
            h
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
    fn focus_paints_the_offset_ring_in_the_beui_ring_token() {
        let mut h = Harness::new(false);
        assert_eq!(h.paint().strokes.len(), 1, "the border only, at rest");

        h.dispatch(&pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 2, "border + ring");
        let (bbox, width, color) = rec.strokes[1];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color, BeuiTokens::resolve_ring(None, Some(&crate::theme())));
        assert!(bbox.x0 <= -RADIO_RING_OFFSET);
        assert!(bbox.x1 >= RADIO_SIZE + RADIO_RING_OFFSET);
    }

    #[test]
    fn a_move_resolves_the_pointer_cursor_and_not_allowed_when_disabled() {
        let mut h = Harness::new(false);
        h.dispatch(&pointer(PointerPhase::Move, 10.0, 10.0));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        let mut disabled = Harness::new(true);
        disabled.dispatch(&pointer(PointerPhase::Move, 10.0, 10.0));
        assert_eq!(disabled.root.cursor(), style::DISABLED_CURSOR);
    }

    #[test]
    fn semantics_reports_a_radio_button_node_with_its_selection() {
        let h = Harness::new(false);
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton)
            .expect("a Role::RadioButton node");
        assert_eq!(node.label(), Some("weekly"));
        assert!(!node.is_selected().unwrap_or(false));
        assert!(node.supports_action(Action::Click));
    }
}
