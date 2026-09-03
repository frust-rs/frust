//! Ports beUI's **Checkbox** — the box whose tick is *drawn on*, stroke by
//! stroke, rather than appearing.
//!
//! Source: `components/motion/checkbox.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `checkbox`: *"Form choice control with a draw-on checkmark, spring
//! press feedback and indeterminate state support."*
//!
//! | class / prop | here |
//! |---|---|
//! | `h-5 w-5 rounded-md border-2` | [`CHECKBOX_SIZE`], [`style::RADIUS_MD`], [`CHECKBOX_BORDER_WIDTH`] |
//! | unchecked `border-muted-foreground/50 bg-background` | `on_surface_variant` at [`BORDER_IDLE_ALPHA`] over `surface` |
//! | `hover:border-muted-foreground` | the same token at full alpha |
//! | checked `border-primary bg-primary text-primary-foreground` | `primary` fill, `on_primary` mark |
//! | `transition-colors duration-200` | [`switch::TRACK_COLOR_RAMP`] |
//! | mark `width=12 viewBox="0 0 24 24" strokeWidth=3` | [`CHECKBOX_MARK_SIZE`] / [`ICON_VIEWBOX`] / [`MARK_STROKE_VIEWBOX`] |
//! | `pathLength: 0 → 1`, `duration: 0.3` (`0.2` mixed), `delay: 0.04` | [`CHECK_DRAW_MS`] / [`MINUS_DRAW_MS`] / [`DRAW_DELAY_MS`] |
//! | svg `opacity/scale 0.5 → 1`, `duration: 0.16` | [`MARK_POP_MS`], [`MARK_POP_FROM`] |
//! | `whileTap={{ scale: 0.92 }}` + `SPRING_PRESS` | [`CHECKBOX_PRESS_SCALE`] |
//! | `focus-visible:ring-2 ring-ring ring-offset-2` | [`switch::draw_focus_ring`] at [`CHECKBOX_RING_OFFSET`] |
//! | `disabled:opacity-60 disabled:cursor-not-allowed` | [`DISABLED_OPACITY`] + [`style::DISABLED_CURSOR`] |
//!
//! # The draw-on, and why it is a path walk
//!
//! Upstream animates SVG `pathLength`, which reveals a stroke by arc length.
//! Both marks are polylines (`M5 13l4 4L19 7` and `M6 12h12`), so the port
//! walks their vertices and emits the prefix of the polyline covering
//! `progress` of the total length — [`mark_path`]. That is the same curve the
//! browser draws rather than an approximation of it, because a polyline's arc
//! length is exactly the sum of its segment lengths.
//!
//! # Controlled, never self-mutating
//!
//! A release inside the box reports `on_checked_change(state, !checked)` — and
//! that is `!checked` **from the mixed state too**, which is upstream's own
//! `onClick={() => onCheckedChange(!checked)}` with no third-value resolution.
//! (The sibling shadcn catalog resolves a mixed box upward to `true` because
//! Radix's primitive does; beUI has no Radix underneath and does not.) The
//! widget's `checked`/`indeterminate` change only when the next `rebuild` feeds
//! the app-confirmed values back down.
//!
//! # Degradation
//!
//! The mark's **exit blur** (`exit={{ …, filter: "blur(4px)" }}`) is not
//! ported: this scene's paint vocabulary has no blur filter, so the exit is the
//! same opacity-and-scale collapse with the blur dropped. Its timing and its
//! `0.5` scale target are upstream's.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    Role, RoundedRect, SemanticsCtx, Shape, Size, Toggled, View, Widget, erase_callback_arg,
};
use frust::{FrameTime, Theme};

use super::switch::{self, Lane, draw_focus_ring, inside, is_activation_key, lerp_color, presses};
use crate::motion::Ramp;
use crate::style;
use crate::tokens::BeuiTokens;
use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};

/// Box edge, in logical px (`h-5 w-5`).
pub const CHECKBOX_SIZE: f64 = 20.0;

/// Border width, in logical px (`border-2`) — twice the catalog's hairline,
/// which is what gives an unchecked beUI box its blocky look.
pub const CHECKBOX_BORDER_WIDTH: f64 = 2.0;

/// Mark box edge, in logical px (`width="12" height="12"`).
pub const CHECKBOX_MARK_SIZE: f64 = 12.0;

/// Side of the SVG viewBox the mark paths are authored in (`viewBox="0 0 24
/// 24"`).
pub const ICON_VIEWBOX: f64 = 24.0;

/// The mark's stroke width in viewBox units (`strokeWidth={3}`), scaled to
/// [`CHECKBOX_MARK_SIZE`] at paint time.
pub const MARK_STROKE_VIEWBOX: f64 = 3.0;

/// The check mark's vertices, in viewBox units: `M5 13l4 4L19 7`.
pub const CHECK_POINTS: [Point; 3] = [
    Point::new(5.0, 13.0),
    Point::new(9.0, 17.0),
    Point::new(19.0, 7.0),
];

/// The indeterminate mark's vertices, in viewBox units: `M6 12h12`.
pub const MINUS_POINTS: [Point; 2] = [Point::new(6.0, 12.0), Point::new(18.0, 12.0)];

/// How long the check draws itself in, in milliseconds (`duration: 0.3`).
pub const CHECK_DRAW_MS: f64 = 300.0;
/// How long the indeterminate minus draws itself in, in milliseconds
/// (`duration: indeterminate ? 0.2 : 0.3`).
pub const MINUS_DRAW_MS: f64 = 200.0;
/// Delay before the draw-on starts, in milliseconds (`delay: 0.04`) — the beat
/// that lets the box's fill land first.
pub const DRAW_DELAY_MS: f64 = 40.0;

/// How long the mark's pop (opacity and scale) takes, in milliseconds
/// (`transition={{ duration: 0.16, ease: EASE_OUT }}`).
pub const MARK_POP_MS: f64 = 160.0;
/// The scale the mark pops from, and collapses back to on exit
/// (`initial`/`exit` `{{ scale: 0.5 }}`).
pub const MARK_POP_FROM: f64 = 0.5;

/// [`MARK_POP_MS`] as the ramp the pop lane runs on.
const MARK_POP_RAMP: Ramp = Ramp::eased(Duration::from_millis(160), EASE_OUT);

/// The scale a pressed box shrinks to (`whileTap={{ scale: 0.92 }}`).
pub const CHECKBOX_PRESS_SCALE: f64 = 0.92;

/// Gap between the box and its focus ring, in logical px (`ring-offset-2`).
pub const CHECKBOX_RING_OFFSET: f64 = 2.0;

/// Alpha of the idle unchecked border (`border-muted-foreground/50`); a hover
/// takes it to full (`hover:border-muted-foreground`).
pub const BORDER_IDLE_ALPHA: f32 = 0.5;

/// Opacity of a disabled checkbox: `disabled:opacity-60`.
const DISABLED_OPACITY: f32 = 0.6;

/// Unthemed fallback ink — the light table's `--primary`.
const FALLBACK_PRIMARY: Color = crate::BEUI_LIGHT.primary;
/// Unthemed fallback mark — the light table's `--primary-foreground`.
const FALLBACK_PRIMARY_FOREGROUND: Color = crate::BEUI_LIGHT.primary_foreground;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback box fill — the light table's `--background`.
const FALLBACK_BACKGROUND: Color = crate::BEUI_LIGHT.background;

/// The prefix of the polyline through `points` covering `progress` of its total
/// arc length, scaled from the [`ICON_VIEWBOX`] into a `size`-square box —
/// upstream's animated `pathLength`, exactly.
///
/// A `progress` of `0.0` (or a degenerate point list) yields an empty path, so
/// a scene never sees a stroke with no extent.
pub fn mark_path(points: &[Point], size: f64, progress: f64) -> BezPath {
    let mut path = BezPath::new();
    let progress = progress.clamp(0.0, 1.0);
    if points.len() < 2 || progress <= 0.0 {
        return path;
    }
    let scale = size / ICON_VIEWBOX;
    let at = |p: Point| Point::new(p.x * scale, p.y * scale);
    let lengths: Vec<f64> = points.windows(2).map(|w| (w[1] - w[0]).hypot()).collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return path;
    }
    let mut remaining = total * progress;
    path.move_to(at(points[0]));
    for (index, length) in lengths.iter().enumerate() {
        if remaining >= *length {
            path.line_to(at(points[index + 1]));
            remaining -= *length;
        } else {
            let from = points[index];
            let to = points[index + 1];
            path.line_to(at(from + (to - from) * (remaining / *length)));
            break;
        }
    }
    path
}

/// The draw-on's own little state machine, kept beside the pop lane because its
/// 40ms delay and its state-dependent duration do not fit a plain `from → to`
/// [`Lane`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Draw {
    /// Nothing to draw — the box carries no mark.
    Absent,
    /// A mark just appeared; `paint` stamps the start (neither a `BuildCtx` nor
    /// an `EventCtx` carries a clock).
    Armed,
    /// Drawing, since this frame.
    Running(FrameTime),
    /// Fully drawn — either the run finished, or the box mounted already marked
    /// (`<AnimatePresence initial={false}>` plays no entry on mount).
    Done,
}

/// A view-held, typed change callback (erased on build).
type OnCheckedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI checkbox. See the [module docs](self).
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
/// Upstream wraps the box and its `label` in one `<label>` element rather than
/// painting text inside the control; compose the visible text alongside (a
/// `Row` of `checkbox(..)` plus `frust::text(..)`) and name the control for
/// assistive tech with [`CheckboxView::label`].
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
    /// Put the box in the mixed state: the filled treatment under a minus mark
    /// and [`Toggled::Mixed`] semantics (`aria-checked="mixed"`).
    ///
    /// Takes precedence over `checked` for everything it paints and announces.
    /// It does **not** change what an activation reports — upstream's own
    /// handler is an unconditional `onCheckedChange(!checked)`.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    /// Disable the control: 60% opacity, inert, not-allowed cursor.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech (the box paints no text of its own).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The resolved checkbox palette.
struct CheckboxColors {
    /// `border-muted-foreground/50`, or full alpha while hovered.
    border_idle: Color,
    /// `bg-background`, the unchecked fill.
    fill_idle: Color,
    /// `border-primary`/`bg-primary`, the marked border and fill.
    primary: Color,
    /// `text-primary-foreground`, the mark.
    mark: Color,
    /// `--ring`, the focus ring.
    ring: Color,
}

/// Resolve the palette, falling back to the vendored light table with no theme
/// threaded.
fn resolve_colors(theme: Option<&Theme>, hovered: bool) -> CheckboxColors {
    let alpha = if hovered { 1.0 } else { BORDER_IDLE_ALPHA };
    let ring = BeuiTokens::resolve_ring(None, theme);
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            CheckboxColors {
                border_idle: style::with_alpha(scheme.on_surface_variant, alpha),
                fill_idle: scheme.surface,
                primary: scheme.primary,
                mark: scheme.on_primary,
                ring,
            }
        }
        None => CheckboxColors {
            border_idle: style::with_alpha(FALLBACK_MUTED_FOREGROUND, alpha),
            fill_idle: FALLBACK_BACKGROUND,
            primary: FALLBACK_PRIMARY,
            mark: FALLBACK_PRIMARY_FOREGROUND,
            ring,
        },
    }
}

impl<State: 'static> View<State> for CheckboxView<State> {
    type Element = CheckboxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CheckboxWidget {
        let marked = self.checked || self.indeterminate;
        let rest = if marked { 1.0 } else { 0.0 };
        CheckboxWidget {
            checked: self.checked,
            indeterminate: self.indeterminate,
            disabled: self.disabled,
            label: self.label.clone(),
            blend: Lane::at_rest(switch::TRACK_COLOR_RAMP, rest),
            pop: Lane::at_rest(MARK_POP_RAMP, rest),
            press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            // `initial={false}`: a box that mounts marked is simply drawn.
            draw: if marked { Draw::Done } else { Draw::Absent },
            hovered: false,
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
        let was_marked = prev.checked || prev.indeterminate;
        let marked = self.checked || self.indeterminate;
        if prev.checked != self.checked || prev.indeterminate != self.indeterminate {
            // The app is the source of truth for both values.
            element.checked = self.checked;
            element.indeterminate = self.indeterminate;
            flags |= ChangeFlags::PAINT;
        }
        if was_marked != marked {
            let target = if marked { 1.0 } else { 0.0 };
            element.blend.retarget(target);
            element.pop.retarget(target);
            if marked {
                element.draw = Draw::Armed;
            }
            // A mark on its way out keeps its drawn path while it fades, the
            // way upstream's exiting `<motion.path>` does; `paint` forgets it
            // once the pop lane reaches zero.
            flags |= ChangeFlags::PAINT;
        } else if marked && prev.indeterminate != self.indeterminate {
            // Swapping check for minus (or back) restarts the draw: upstream
            // keys the `<motion.svg>` on the state, so the old mark unmounts
            // and the new one plays its own entry.
            element.draw = Draw::Armed;
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

/// The retained widget for a [`CheckboxView`].
pub struct CheckboxWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    /// The app-confirmed mixed state, which outranks `checked` everywhere it is
    /// painted or announced.
    indeterminate: bool,
    disabled: bool,
    label: Option<String>,
    /// The border/fill crossfade between the unmarked and marked treatments.
    blend: Lane,
    /// The mark's presence — its opacity and its pop scale.
    pop: Lane,
    /// The press shrink, `0.0` .. `1.0`.
    press: Lane,
    /// The draw-on's stage.
    draw: Draw,
    /// The latched hover, self-corrected from `PaintCtx::is_hovered` each paint.
    hovered: bool,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`.
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

    /// Whether the box paints its filled treatment: checked *or* mixed.
    fn marked(&self) -> bool {
        self.checked || self.indeterminate
    }

    /// The mark's vertices: the minus outranks the check, the way
    /// `data-state="indeterminate"` outranks `"checked"` upstream.
    fn mark_points(&self) -> &'static [Point] {
        if self.indeterminate {
            &MINUS_POINTS
        } else {
            &CHECK_POINTS
        }
    }

    /// How long this mark takes to draw itself in, in milliseconds.
    fn draw_duration_ms(&self) -> f64 {
        if self.indeterminate {
            MINUS_DRAW_MS
        } else {
            CHECK_DRAW_MS
        }
    }

    /// Advance the draw-on to `now`, returning `(progress, owes_frame)`.
    fn advance_draw(&mut self, now: FrameTime) -> (f64, bool) {
        match self.draw {
            Draw::Absent => (0.0, false),
            Draw::Done => (1.0, false),
            Draw::Armed => {
                self.draw = Draw::Running(now);
                (0.0, true)
            }
            Draw::Running(start) => {
                let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
                let span = self.draw_duration_ms();
                if elapsed >= DRAW_DELAY_MS + span {
                    self.draw = Draw::Done;
                    return (1.0, false);
                }
                let t = ((elapsed - DRAW_DELAY_MS) / span).clamp(0.0, 1.0);
                (EASE_OUT.transform(t), true)
            }
        }
    }
}

impl Widget for CheckboxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() || self.disabled {
            self.hovered = false;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme, self.hovered && !self.marked());
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();
        let now = ctx.frame_time();

        let mut owes_frame = false;
        let draw = if reduce {
            self.blend.snap();
            self.pop.snap();
            self.press.snap();
            self.draw = if self.marked() {
                Draw::Done
            } else {
                Draw::Absent
            };
            if self.marked() { 1.0 } else { 0.0 }
        } else {
            owes_frame |= self.blend.advance(now);
            owes_frame |= self.pop.advance(now);
            owes_frame |= self.press.advance(now);
            let (progress, owes) = self.advance_draw(now);
            owes_frame |= owes;
            progress
        };
        let pop = self.pop.value().clamp(0.0, 1.0);
        if pop <= 0.0 && !self.marked() {
            // The exit finished: the next mark starts from an empty path.
            self.draw = Draw::Absent;
        }

        let box_size = Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE);
        let origin = ctx.origin();
        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);
        let blend = self.blend.value().clamp(0.0, 1.0);

        // `whileTap` scales the whole control about its own centre.
        let press = self.press.value().clamp(0.0, 1.0);
        let scale = 1.0 - (1.0 - CHECKBOX_PRESS_SCALE) * press;
        let pivot = origin + (box_size.to_vec2() / 2.0);
        scene.push_transform(
            Affine::translate(pivot.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-pivot.to_vec2()),
        );

        scene.fill_rounded_rect(
            origin,
            box_size,
            style::RADIUS_MD,
            tint(lerp_color(colors.fill_idle, colors.primary, blend)),
        );

        // `border-2` is inside the box (`box-sizing: border-box`), so the
        // stroke is centred half a border in from the edge.
        let inset = CHECKBOX_BORDER_WIDTH / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, box_size).inset(-inset),
            (style::RADIUS_MD - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&outline, style::PATH_TOLERANCE),
            CHECKBOX_BORDER_WIDTH,
            &Brush::Solid(tint(lerp_color(colors.border_idle, colors.primary, blend))),
        );

        if pop > 0.0 && draw > 0.0 {
            let path = mark_path(self.mark_points(), CHECKBOX_MARK_SIZE, draw);
            let mark_inset = (CHECKBOX_SIZE - CHECKBOX_MARK_SIZE) / 2.0;
            let mark_centre = origin + (box_size.to_vec2() / 2.0);
            // One lane drives the mark's opacity and its scale, the way
            // upstream's single `initial`/`animate` pair does. The layer states
            // the alpha in the untransformed space the mark was measured in.
            let mark_scale = MARK_POP_FROM + (1.0 - MARK_POP_FROM) * pop;
            scene.push_layer(origin, box_size, pop as f32);
            scene.push_transform(
                Affine::translate(mark_centre.to_vec2())
                    * Affine::scale(mark_scale)
                    * Affine::translate(-mark_centre.to_vec2()),
            );
            scene.stroke_path(
                Point::new(origin.x + mark_inset, origin.y + mark_inset),
                &path,
                MARK_STROKE_VIEWBOX * CHECKBOX_MARK_SIZE / ICON_VIEWBOX,
                &Brush::Solid(tint(colors.mark)),
            );
            scene.pop_transform();
            scene.pop_layer();
        }

        scene.pop_transform();

        if focused {
            draw_focus_ring(
                scene,
                origin,
                box_size,
                style::RADIUS_MD,
                CHECKBOX_RING_OFFSET,
                tint(colors.ring),
            );
        }

        // Paint-only animation (the box never resizes), so a bare frame request
        // is the right one — never `request_layout`.
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
                (self.on_checked_change)(ctx, !self.checked);
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
                        // the box, and let the press scale go off it.
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
                            (self.on_checked_change)(ctx, !self.checked);
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
    use frust::authoring::{
        EventOutcome, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use std::any::Any;

    /// Records the fills, stroked paths (bbox + width + color), layer alphas
    /// and transform pushes this widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        layers: Vec<f32>,
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
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
    }

    impl Recorder {
        /// The mark stroke of the pass, if any — the one stroke narrower than
        /// the border.
        fn mark(&self) -> Option<(Rect, f64, Color)> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w < CHECKBOX_BORDER_WIDTH)
                .copied()
        }

        /// The border stroke of the pass.
        fn border(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w == CHECKBOX_BORDER_WIDTH)
                .expect("the 2px border was stroked")
        }
    }

    #[derive(Default)]
    struct Toggles {
        last: Option<bool>,
        count: u32,
    }

    const BOX: Size = Size::new(CHECKBOX_SIZE, CHECKBOX_SIZE);

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
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

    fn paint_at(w: &mut CheckboxWidget, theme: Option<&Theme>, millis: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, BOX, ft_ms(millis));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn paint(w: &mut CheckboxWidget, theme: Option<&Theme>) -> Recorder {
        paint_at(w, theme, 0.0).0
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
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        w.event(&mut ctx, event)
    }

    /// The scale from viewBox units into the painted mark box.
    const MARK_SCALE: f64 = CHECKBOX_MARK_SIZE / ICON_VIEWBOX;

    // ---- The draw-on path -------------------------------------------------

    #[test]
    fn the_mark_path_reveals_the_polyline_by_arc_length() {
        let full = mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, 1.0);
        let bbox = full.bounding_box();
        // The whole `M5 13l4 4L19 7`, scaled into the 12px mark box.
        assert!((bbox.x0 - 5.0 * MARK_SCALE).abs() < 1e-9);
        assert!((bbox.x1 - 19.0 * MARK_SCALE).abs() < 1e-9);
        assert!((bbox.y0 - 7.0 * MARK_SCALE).abs() < 1e-9);
        assert!((bbox.y1 - 17.0 * MARK_SCALE).abs() < 1e-9);

        // Nothing at all before the draw starts...
        assert!(mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, 0.0).is_empty());
        // ...and a strictly growing prefix through the run.
        let mut previous = 0.0;
        for step in 1..=20 {
            let progress = f64::from(step) / 20.0;
            let drawn = mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, progress).bounding_box();
            let reach = drawn.width() + drawn.height();
            assert!(reach > previous, "the pen went backwards at {progress}");
            previous = reach;
        }
    }

    #[test]
    fn the_short_leg_of_the_check_is_drawn_before_the_long_one() {
        // `M5 13l4 4` is 4·√2 long and `L19 7` is 10·√2 — so the corner is
        // reached at 2/7 of the total, not at half.
        let corner = 4.0 / 14.0;
        let at_corner = mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, corner).bounding_box();
        assert!(
            (at_corner.x1 - 9.0 * MARK_SCALE).abs() < 1e-9,
            "the pen is exactly at the vertex"
        );
        // Just short of it, it has not turned the corner yet.
        let before = mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, corner * 0.5).bounding_box();
        assert!(before.x1 < 9.0 * MARK_SCALE);
    }

    #[test]
    fn the_minus_path_is_a_flat_horizontal_run() {
        let bbox = mark_path(&MINUS_POINTS, CHECKBOX_MARK_SIZE, 1.0).bounding_box();
        assert!(bbox.height() < 1e-9, "`M6 12h12` has no vertical extent");
        assert!((bbox.width() - 12.0 * MARK_SCALE).abs() < 1e-9);
        // Half-drawn is exactly half as wide.
        let half = mark_path(&MINUS_POINTS, CHECKBOX_MARK_SIZE, 0.5).bounding_box();
        assert!((half.width() - 6.0 * MARK_SCALE).abs() < 1e-9);
    }

    #[test]
    fn a_degenerate_point_list_draws_nothing_rather_than_panicking() {
        assert!(mark_path(&[], CHECKBOX_MARK_SIZE, 1.0).is_empty());
        assert!(mark_path(&CHECK_POINTS[..1], CHECKBOX_MARK_SIZE, 1.0).is_empty());
        let doubled = [Point::new(3.0, 3.0), Point::new(3.0, 3.0)];
        assert!(mark_path(&doubled, CHECKBOX_MARK_SIZE, 1.0).is_empty());
        // Out-of-range progress clamps rather than running off the polyline.
        assert_eq!(
            mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, 4.0).bounding_box(),
            mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, 1.0).bounding_box()
        );
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn an_unchecked_box_paints_a_background_fill_and_a_half_alpha_border() {
        let mut w = widget(false, false);
        let rec = paint(&mut w, None);
        assert_eq!(rec.rrects.len(), 1);
        let (_, size, radius, fill) = rec.rrects[0];
        assert_eq!(size, BOX);
        assert_eq!(radius, style::RADIUS_MD, "rounded-md");
        assert_eq!(fill, FALLBACK_BACKGROUND, "bg-background");

        let (_, width, color) = rec.border();
        assert_eq!(width, CHECKBOX_BORDER_WIDTH);
        assert_eq!(color.components[3], BORDER_IDLE_ALPHA);
        assert!(rec.mark().is_none(), "nothing to draw");
    }

    #[test]
    fn a_box_that_mounts_checked_is_already_drawn_and_owes_no_frame() {
        let theme = crate::theme();
        let mut w = widget(true, false);
        let (rec, owes) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes, "`initial={{false}}` plays no entry on mount");
        assert_eq!(rec.rrects[0].3, theme.scheme().primary, "bg-primary");
        assert_eq!(rec.border().2, theme.scheme().primary, "border-primary");

        let (bbox, width, color) = rec.mark().expect("the check is stroked");
        assert_eq!(color, theme.scheme().on_primary);
        assert!((width - MARK_STROKE_VIEWBOX * MARK_SCALE).abs() < 1e-9);
        // The 12px mark sits centred in the 20px box.
        let inset = (CHECKBOX_SIZE - CHECKBOX_MARK_SIZE) / 2.0;
        assert!(bbox.x0 >= inset - 1e-9 && bbox.x1 <= CHECKBOX_SIZE - inset + 1e-9);
        assert_eq!(rec.layers, vec![1.0], "fully present");
    }

    #[test]
    fn the_mixed_state_paints_a_minus_and_outranks_checked() {
        let theme = crate::theme();
        let mut counter = 0u64;
        let mixed = view(true, false).indeterminate(true);
        let mut w = View::<Toggles>::build(&mixed, &mut BuildCtx::new(&mut counter));
        let rec = paint(&mut w, Some(&theme));
        let (bbox, ..) = rec.mark().expect("the minus is stroked");
        assert!(
            bbox.height() < 1e-9,
            "a checked box that is also mixed still paints the minus"
        );
    }

    #[test]
    fn disabled_paint_dims_every_painted_alpha_to_sixty_percent() {
        let theme = crate::theme();
        let mut enabled = widget(true, false);
        let mut disabled = widget(true, true);
        let on = paint(&mut enabled, Some(&theme));
        let off = paint(&mut disabled, Some(&theme));
        assert!(
            (off.rrects[0].3.components[3] - on.rrects[0].3.components[3] * DISABLED_OPACITY).abs()
                < 1e-6
        );
        assert!(
            (off.mark().expect("mark").2.components[3]
                - on.mark().expect("mark").2.components[3] * DISABLED_OPACITY)
                .abs()
                < 1e-6
        );
    }

    // ---- Motion -----------------------------------------------------------

    #[test]
    fn checking_a_box_pops_the_mark_and_draws_it_on_after_the_delay() {
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        // The first paint stamps the draw's start: the fill has begun, but the
        // pen has not moved.
        let (start, owes) = paint_at(&mut w, None, 0.0);
        assert!(owes);
        assert!(start.mark().is_none(), "nothing drawn on the delay beat");
        assert_ne!(start.rrects[0].3, FALLBACK_PRIMARY, "the fill is blending");

        // Partway through, a prefix of the check is on screen.
        let (mid, owes) = paint_at(&mut w, None, DRAW_DELAY_MS + CHECK_DRAW_MS * 0.25);
        assert!(owes);
        let (mid_bbox, ..) = mid.mark().expect("a partial check");
        let full = mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, 1.0).bounding_box();
        assert!(
            mid_bbox.width() < full.width(),
            "a prefix, not the whole path"
        );

        // Settled: whole path, full alpha, no more frames owed.
        let (end, owes) = paint_at(&mut w, None, 3_000.0);
        assert!(!owes);
        assert_eq!(end.layers, vec![1.0]);
        let (end_bbox, ..) = end.mark().expect("the whole check");
        assert!((end_bbox.width() - full.width()).abs() < 1e-9);
        assert_eq!(end.rrects[0].3, FALLBACK_PRIMARY, "landed on-token");
    }

    #[test]
    fn unchecking_fades_the_mark_out_and_then_forgets_it() {
        let mut counter = 0u64;
        let prev = view(true, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(false, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        paint_at(&mut w, None, 0.0);
        let (mid, owes) = paint_at(&mut w, None, MARK_POP_MS / 2.0);
        assert!(owes);
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "mid-fade alpha {alpha}");
        assert!(mid.mark().is_some(), "still drawn while it fades");

        let (end, owes) = paint_at(&mut w, None, 3_000.0);
        assert!(!owes);
        assert!(end.mark().is_none(), "gone");
        assert_eq!(
            w.draw,
            Draw::Absent,
            "and forgotten, so the next mark re-draws"
        );
    }

    #[test]
    fn reduce_motion_shows_the_finished_mark_and_owes_no_frame() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (rec, owes) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes, "reduce_motion asks for no animation frame");
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);
        let full = mark_path(&CHECK_POINTS, CHECKBOX_MARK_SIZE, 1.0).bounding_box();
        assert!((rec.mark().expect("drawn").0.width() - full.width()).abs() < 1e-9);
        assert_eq!(rec.layers, vec![1.0]);
    }

    #[test]
    fn a_held_box_shrinks_under_the_press_spring() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        paint_at(&mut w, None, 0.0);
        let (rec, owes) = paint_at(&mut w, None, 30.0);
        assert!(owes);
        let scale = rec.transforms[0].as_coeffs()[0];
        assert!(
            (CHECKBOX_PRESS_SCALE..1.0).contains(&scale),
            "press scale {scale}"
        );
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_the_requested_value_without_self_mutating() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.last, Some(true));
        assert!(!w.checked, "the app owns `checked`");
        assert!(!w.captured);
    }

    #[test]
    fn a_mixed_box_reports_the_plain_negation_upstream_reports() {
        let mut counter = 0u64;
        let mixed = view(false, false).indeterminate(true);
        let mut w = View::<Toggles>::build(&mixed, &mut BuildCtx::new(&mut counter));
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            state.last,
            Some(true),
            "`onCheckedChange(!checked)`, with no third-value resolution"
        );
        assert!(w.indeterminate, "the app owns the mixed state too");
    }

    #[test]
    fn up_outside_cancel_and_a_secondary_press_never_fire() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
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
    fn space_activates_and_a_disabled_checkbox_is_inert() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        assert_eq!(dispatch(&mut w, &mut state, &space()), EventResult::Handled);
        assert_eq!(state.last, Some(true));

        let mut disabled = widget(false, true);
        assert_eq!(
            dispatch(&mut disabled, &mut state, &space()),
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
        assert_eq!(state.count, 1, "only the enabled box reported");
    }

    #[test]
    fn a_hover_brightens_the_unchecked_border_and_the_paint_pass_corrects_it() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        w.event(&mut ctx, &pointer(PointerPhase::Move, 10.0, 10.0));
        assert!(w.hovered);
        assert!(ctx.needs_redraw(), "the border alpha changed");

        // The paint pass is authoritative: with no hover on the path the latch
        // self-corrects and the border returns to half alpha.
        let rec = paint(&mut w, None);
        assert_eq!(rec.border().2.components[3], BORDER_IDLE_ALPHA);
        assert!(!w.hovered);
    }

    #[test]
    fn rebuild_adopts_the_confirmed_values_and_disarms_on_disable() {
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
        assert_eq!(w.draw, Draw::Armed, "the mark will draw itself on");
        assert!(flags.needs_paint());

        // Swapping check for minus while marked restarts the draw.
        let mixed = view(true, true).indeterminate(true);
        View::<Toggles>::rebuild(&mixed, &next, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.indeterminate);
        assert_eq!(w.draw, Draw::Armed);
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    /// One checkbox under a real `RenderRoot` — the only harness that can
    /// exercise focus and the cursor.
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
            let (disabled, indeterminate) = (self.disabled, self.indeterminate);
            let mut logic =
                move |_s: &mut Toggles| view(false, disabled).indeterminate(indeterminate);
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
        assert!(bbox.x0 <= -CHECKBOX_RING_OFFSET);
        assert!(bbox.x1 >= CHECKBOX_SIZE + CHECKBOX_RING_OFFSET);
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
    fn semantics_reports_a_checkbox_node_and_the_mixed_state_as_mixed() {
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

        h.indeterminate = true;
        h.rebuild();
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::CheckBox)
            .expect("a Role::CheckBox node");
        assert_eq!(node.toggled(), Some(Toggled::Mixed), "aria-checked=mixed");
    }

    /// The ramp the pop lane runs on is the millisecond figure the module
    /// documents, not a number that drifted from it.
    #[test]
    fn the_pop_ramp_matches_the_documented_duration() {
        assert_eq!(
            MARK_POP_RAMP,
            Ramp::eased(Duration::from_secs_f64(MARK_POP_MS / 1000.0), EASE_OUT)
        );
        const {
            assert!(
                MINUS_DRAW_MS < CHECK_DRAW_MS,
                "the minus is the quicker mark"
            )
        };
    }
}
